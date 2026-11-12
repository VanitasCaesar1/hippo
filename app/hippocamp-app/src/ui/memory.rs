//! Memory: every note as a tile of a treemap grouped by folder, painted on
//! one canvas, with a cost strip above it, five modes (Reads, Injected,
//! Ignored, Size, Edits), a side panel for the selected note, a filter and a
//! sorted list one key away.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{
    Bounds, Context, FocusHandle, FontWeight, Hsla, Pixels, Point, Role, SharedString, TextRun,
    Window, canvas, div, fill, point, prelude::*, px, quad, size,
};
use hippocamp_core::xray::{PartKind, human_bytes};
use hippocamp_view::memory::{self, Fresh, Laid, Map, Rect, Shade, SizeBy, Tile};

use super::Shell;
use super::kit::{self, Kind};
use super::theme::Look;
use crate::flow::Purpose;
use crate::state::Tab;

/// The last paint's canvas bounds and layout, shared with the pointer
/// handlers.
pub type LaidCell = Rc<RefCell<Option<(Bounds<Pixels>, Laid)>>>;

/// A cost-strip filter: the map dims every note it does not keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostFilter {
    /// Required notes only.
    Required,
    /// Notes whose review is overdue.
    Overdue,
}

/// The Memory section's state.
pub struct MemoryUi {
    pub by: SizeBy,
    /// The sorted list instead of the map.
    pub list: bool,
    /// The selected tile's note.
    pub selected: Option<PathBuf>,
    /// The tile under the pointer and the pointer, in window coordinates.
    pub hover: Option<(usize, Point<Pixels>)>,
    /// The last paint's canvas bounds and layout, for hit testing.
    pub laid: LaidCell,
    pub focus: FocusHandle,
    pub filter: Option<CostFilter>,
}

impl MemoryUi {
    pub fn new(focus: FocusHandle) -> MemoryUi {
        MemoryUi {
            by: SizeBy::Reads,
            list: false,
            selected: None,
            hover: None,
            laid: Rc::new(RefCell::new(None)),
            focus,
            filter: None,
        }
    }
}

/// Below this pane width the side panel becomes a strip under the map.
// The side panel from 900 points of pane (a 1200-point window).
const SIDE_PANEL: f32 = 900.;
/// Below this window width the toggle uses its short words.
const SHORT_WORDS: f32 = 900.;

/// A shade's fill in mode `by`. Amber (the accent) only in Reads, where
/// the colour is freshness; Edits has its own hue, the info colour;
/// the categorical modes use the status colours.
pub fn shade_color(look: &Look, s: Shade, by: SizeBy) -> Hsla {
    let a = if by == SizeBy::Edits {
        look.c.focus
    } else {
        look.c.text_accent
    };
    match s {
        Shade::Fresh(Fresh::Week) => a.opacity(0.85),
        Shade::Fresh(Fresh::Month) => a.opacity(0.4),
        Shade::Fresh(Fresh::Older) => look.c.hover,
        Shade::Required => look.c.focus.opacity(0.75),
        Shade::Relevant => look.c.focus.opacity(0.35),
        Shade::Opened => kit::ok(look).opacity(0.55),
        Shade::Half => look.c.warning.opacity(0.55),
        Shade::Ignored => look.c.error.opacity(0.6),
        Shade::ManyReads => kit::ok(look).opacity(0.6),
        Shade::FewReads => kit::ok(look).opacity(0.28),
        Shade::Unread => look.c.hover,
    }
}

/// The text colour on a tile of shade `s`.
fn on_color(look: &Look, s: Shade) -> Hsla {
    match s {
        Shade::Fresh(Fresh::Week) => look.c.window,
        _ => look.c.text,
    }
}

fn matches(t: &Tile, filter: &str, cost: Option<CostFilter>) -> bool {
    let kept = match cost {
        Some(CostFilter::Required) => t.required,
        Some(CostFilter::Overdue) => t.overdue.is_some(),
        None => true,
    };
    if !kept {
        return false;
    }
    if filter.is_empty() {
        return true;
    }
    let f = filter.to_lowercase();
    t.title.to_lowercase().contains(&f) || t.path.to_string_lossy().to_lowercase().contains(&f)
}

/// `when` as `today 14:02`, `3 Nov`, `2 days ago`.
pub fn changed_words(t: Option<time::OffsetDateTime>) -> String {
    let Some(t) = t else {
        return "never".into();
    };
    let off = super::status::local_offset();
    let t = t.to_offset(off);
    let today = time::OffsetDateTime::now_utc().to_offset(off).date();
    let days = (today - t.date()).whole_days();
    match days {
        0 => format!("today {:02}:{:02}", t.hour(), t.minute()),
        1 => "yesterday".into(),
        2..=6 => format!("{days} days ago"),
        _ => format!("{} {}", t.day(), &format!("{:?}", t.month())[..3]),
    }
}

impl Shell {
    pub(crate) fn memory_map(&self) -> Option<&Map> {
        self.loaded.as_ref().and_then(|l| l.memory.as_ref())
    }

    /// Opens a tile's note page in Memory's stack.
    pub(crate) fn open_memory_tile(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.memory_ui.selected = Some(path.clone());
        self.memory_ui.hover = None;
        self.state.tabs.open(Tab::Note(path));
        // The map leaves the screen with its focus: keys (Escape) go to the
        // window.
        window.focus(&self.focus, cx);
        self.after_state_change(window, cx);
    }

    /// Reads a note's per-profile reads from the rollup once.
    fn ensure_note_stats(&mut self, path: &Path) {
        if !self.note_stats.contains_key(path)
            && let Some(home) = self
                .loaded
                .as_ref()
                .and_then(|l| l.setup.config())
                .map(|c| c.home.clone())
            && let Some(r) = hippocamp_core::rollup::Rollup::open(&home).ok().flatten()
        {
            let stats = memory::note_stats(
                &r,
                path,
                time::OffsetDateTime::now_utc(),
                super::status::local_offset(),
            );
            self.note_stats.insert(path.to_path_buf(), stats);
        }
    }

    /// Selects a tile: the side panel shows it; its per-profile reads are
    /// read from the rollup once.
    pub(crate) fn select_memory_tile(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.ensure_note_stats(&path);
        if self.memory_ui.selected.as_ref() != Some(&path) {
            self.xray.confirm = None;
            self.xray.tune_result = None;
        }
        self.memory_ui.selected = Some(path);
        cx.notify();
    }

    /// The tile order arrows walk: the map's (groups, then tiles largest
    /// first) or the list's.
    fn memory_order(&self) -> Vec<usize> {
        let Some(map) = self.memory_map() else {
            return Vec::new();
        };
        if self.memory_ui.list {
            map.sorted(self.memory_ui.by)
        } else {
            map.groups(self.memory_ui.by)
                .into_iter()
                .flat_map(|(_, ix)| ix)
                .collect()
        }
    }

    /// Moves the selection `step` tiles in reading order.
    fn memory_step(&mut self, step: i32, cx: &mut Context<Self>) {
        let order = self.memory_order();
        let Some(map) = self.memory_map() else { return };
        if order.is_empty() {
            return;
        }
        let at = self
            .memory_ui
            .selected
            .as_ref()
            .and_then(|p| order.iter().position(|i| map.tiles[*i].path == *p));
        let next = match at {
            None => 0,
            Some(i) => (i as i32 + step).clamp(0, order.len() as i32 - 1) as usize,
        };
        let path = map.tiles[order[next]].path.clone();
        self.select_memory_tile(path, cx);
    }

    pub(crate) fn set_memory_mode(&mut self, by: SizeBy, cx: &mut Context<Self>) {
        self.memory_ui.by = by;
        // A selection the new mode has no tile for falls back to the
        // largest tile.
        if by.injected_only()
            && let Some(map) = self.memory_map()
            && let Some(sel) = self.memory_ui.selected.clone()
            && map.tiles.iter().any(|t| t.path == sel && t.injected == 0)
        {
            self.memory_ui.selected = None;
        }
        cx.notify();
    }

    /// Memory's keys: 1–5 the mode, L the list, arrows select, ↩ opens.
    pub(super) fn memory_key(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match key {
            "1" | "2" | "3" | "4" | "5" => {
                let i = key.parse::<usize>().unwrap_or(1) - 1;
                self.set_memory_mode(SizeBy::ALL[i], cx);
            }
            "l" => self.memory_ui.list = !self.memory_ui.list,
            "down" | "right" | "j" => self.memory_step(1, cx),
            "up" | "left" | "k" => self.memory_step(-1, cx),
            "enter" => {
                if let Some(p) = self.memory_shown().map(|t| t.path.clone()) {
                    self.open_memory_tile(p, window, cx);
                }
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// The note the side panel shows: the selection, else the largest tile
    /// of the current mode.
    fn memory_shown(&self) -> Option<&Tile> {
        let map = self.memory_map()?;
        if let Some(sel) = &self.memory_ui.selected
            && let Some(t) = map.tiles.iter().find(|t| &t.path == sel)
        {
            return Some(t);
        }
        let by = self.memory_ui.by;
        map.groups(by)
            .into_iter()
            .flat_map(|(_, ix)| ix)
            .max_by_key(|i| map.tiles[*i].value(by))
            .map(|i| &map.tiles[i])
    }

    pub(super) fn render_memory_tab(
        &mut self,
        look: &Look,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(map) = self.memory_map().cloned() else {
            return div()
                .p(look.px(20.))
                .text_color(look.c.text_muted)
                .child(if self.loaded.as_ref().is_some_and(|l| l.ready()) {
                    "No notes folder yet. Manage › Give your agents a memory sets one."
                } else {
                    "Reading…"
                })
                .into_any_element();
        };
        let width = f32::from(window.viewport_size().width);
        let pane = width - self.sidebar_width(window);
        let narrow = width < SHORT_WORDS;
        let side = pane >= SIDE_PANEL;
        let by = self.memory_ui.by;
        let filter = self.brain_filter().read(cx).text().to_string();

        let list_on = self.memory_ui.list;
        let _ = narrow;
        let body = if list_on {
            self.memory_list(&map, by, &filter, look, cx)
        } else {
            self.memory_canvas(&map, by, &filter, look, cx)
        };

        // One line in every mode, `review overdue` at its right.
        let legend = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(look.px(14.))
            .gap_y(look.px(4.))
            .flex_shrink_0()
            .text_size(look.text(11.5))
            .text_color(look.c.text_muted)
            .child(div().text_color(kit::dim(look)).child(by.means()))
            .children(by.shades().iter().map(|s| {
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(6.))
                    .child(
                        div()
                            .size(look.px(10.))
                            .rounded(look.px(2.))
                            .bg(shade_color(look, *s, by)),
                    )
                    .child(s.label())
            }))
            // The never-injected block names itself; no swatch.
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(look.px(6.))
                    .child(kit::dot(look, look.c.warn))
                    .child("review overdue"),
            );

        let main = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .gap(look.px(8.))
            .child(body)
            .child(legend);
        let shown = self.memory_shown().cloned();
        if let Some(t) = &shown {
            self.ensure_note_stats(&t.path.clone());
        }
        let row = div()
            .flex()
            .flex_1()
            .min_h_0()
            .gap(look.px(12.))
            .child(main)
            .when(side, |d| {
                d.children(
                    shown
                        .as_ref()
                        .map(|t| self.memory_side_panel(t, &map, look, cx)),
                )
            });

        div()
            .id("memory")
            .key_context("Memory")
            .track_focus(&self.memory_ui.focus)
            .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, window, cx| {
                let k = &e.keystroke;
                if k.modifiers.platform || k.modifiers.control || k.modifiers.alt {
                    return;
                }
                if this.memory_key(k.key.as_str(), window, cx) {
                    cx.stop_propagation();
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .gap(look.px(10.))
            .px(look.px(if narrow { 12. } else { 18. }))
            .pt(look.px(12.))
            .pb(look.px(10.))
            .child(self.memory_cost_strip(&map, pane, look, cx))
            .child(row)
            .when(!side, |d| {
                d.children(shown.as_ref().map(|t| self.memory_strip(t, look, cx)))
            })
            .into_any_element()
    }

    /// Memory's controls in the title bar: the five modes, the
    /// map or list, and the filter.
    pub(crate) fn memory_title_controls(
        &self,
        narrow: bool,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let by = self.memory_ui.by;
        let modes = SizeBy::ALL.iter().fold(kit::segmented(look), |d, b| {
            let b = *b;
            d.child(
                kit::segment(
                    look,
                    SharedString::from(format!("size-{}", b.label())),
                    None,
                    if narrow { b.short() } else { b.label() },
                    b == by,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_memory_mode(b, cx);
                })),
            )
        });
        let list_on = self.memory_ui.list;
        let views = kit::segmented(look)
            .child(
                kit::segment(look, "memory-map", Some("layout-grid"), "", !list_on).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.memory_ui.list = false;
                        cx.notify();
                    }),
                ),
            )
            .child(
                kit::segment(look, "memory-list", Some("list"), "", list_on).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.memory_ui.list = true;
                        cx.notify();
                    },
                )),
            );
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(look.px(8.))
            .child(modes)
            .child(views)
            .child(
                div()
                    .w(look.px(if narrow { 120. } else { 150. }))
                    .child(self.brain_filter().clone()),
            )
            .into_any_element()
    }

    /// Four numbers a developer can act on, each a filter.
    fn memory_cost_strip(
        &self,
        map: &Map,
        pane: f32,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let short = pane < SIDE_PANEL;
        let by = self.memory_ui.by;
        let filter = self.memory_ui.filter;
        let cost = map.cost.clone();
        let card = |id: &'static str, big: String, big_color: Hsla, words: String, on: bool| {
            // Four cells of one card, the figure over its words,
            // hairlines between; the chosen one raised.
            div()
                .id(id)
                .role(Role::Button)
                .aria_selected(on)
                .flex()
                .flex_col()
                .justify_center()
                .gap(look.px(2.))
                .flex_1()
                .min_w_0()
                .h(look.px(if short { 48. } else { 58. }))
                .px(look.px(if short { 10. } else { 14. }))
                .border_l(look.fixed(1.))
                .border_color(look.c.border)
                .when(on, |d| d.bg(look.c.raised))
                .cursor_pointer()
                .hover(|h| h.bg(look.c.hover))
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(look.text(if short { 15. } else { 18. }))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(big_color)
                        .child(big),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(look.text(12.))
                        .text_color(look.c.text_muted)
                        .child(words),
                )
        };
        let mut row = div().flex().flex_1().min_w_0().ml(-look.fixed(1.));
        match &cost {
            Some(c) => {
                row = row
                    .child(
                        card(
                            "cost-block",
                            human_bytes(c.avg_block),
                            look.c.text,
                            if short {
                                "per session".into()
                            } else {
                                format!("start blocks / session · {}", c.sessions)
                            },
                            by == SizeBy::Injected && filter.is_none(),
                        )
                        .tooltip(super::tooltip::Tooltip::text(format!(
                            "What hippocamp's start blocks gave each of {} sessions in the last 7 days, on average (a resumed session can get more than one). Shows the Injected map.",
                            c.sessions
                        )))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.memory_ui.filter = None;
                            this.set_memory_mode(SizeBy::Injected, cx);
                        })),
                    )
                    .child(
                        card(
                            "cost-ignored",
                            format!("{}%", c.ignored_percent),
                            if c.ignored_percent > 50 {
                                look.c.error
                            } else {
                                look.c.text
                            },
                            if short {
                                "never opened".into()
                            } else {
                                "of it never opened or cited".into()
                            },
                            by == SizeBy::Ignored && filter.is_none(),
                        )
                        .tooltip(super::tooltip::Tooltip::text(
                            "Bytes of those blocks no tool call opened and no reply cited. Shows the Ignored map.",
                        ))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.memory_ui.filter = None;
                            this.set_memory_mode(SizeBy::Ignored, cx);
                        })),
                    );
            }
            None => {
                row = row.child(
                    div()
                        .flex()
                        .items_center()
                        .flex_1()
                        .min_w_0()
                        .h(look.px(if short { 48. } else { 58. }))
                        .px(look.px(14.))
                        .truncate()
                        .text_size(look.text(12.))
                        .text_color(look.c.text_muted)
                        .child("no start blocks recorded this week"),
                );
            }
        }
        let mostly = map.mostly_ignored_required();
        let req_on = filter == Some(CostFilter::Required);
        let od_on = filter == Some(CostFilter::Overdue);
        row = row
            .child(
                card(
                    "cost-required",
                    map.required.to_string(),
                    look.c.text,
                    if short {
                        format!("required · {mostly} ignored")
                    } else {
                        format!("required notes · {mostly} mostly ignored")
                    },
                    req_on,
                )
                .tooltip(super::tooltip::Tooltip::text(
                    "Required notes ride in every start block. Mostly ignored: never opened in at least two of three blocks. Shows them on the Ignored map.",
                ))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.memory_ui.filter = if req_on {
                        None
                    } else {
                        Some(CostFilter::Required)
                    };
                    if !req_on {
                        this.set_memory_mode(SizeBy::Ignored, cx);
                    }
                    cx.notify();
                })),
            )
            .child(
                card(
                    "cost-overdue",
                    map.overdue.to_string(),
                    if map.overdue > 0 {
                        look.c.warning
                    } else {
                        look.c.text
                    },
                    if short {
                        "overdue".into()
                    } else {
                        "notes overdue for review".into()
                    },
                    od_on,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.memory_ui.filter = if od_on {
                        None
                    } else {
                        Some(CostFilter::Overdue)
                    };
                    cx.notify();
                })),
            );
        kit::card(look)
            .flex()
            .flex_shrink_0()
            .overflow_hidden()
            .child(row)
            .into_any_element()
    }

    /// The selected note's facts, read-by-profile bars and tune tip: the
    /// 320-point side panel at 1000 points of pane and wider.
    fn memory_side_panel(
        &self,
        t: &Tile,
        map: &Map,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let stats = self.note_stats.get(&t.path);
        let week = map.cost.as_ref().map_or(0, |c| c.sessions);
        let fact = |k: &str, v: gpui::AnyElement| {
            div()
                .flex()
                .items_start()
                .gap(look.px(10.))
                .py(look.px(3.))
                .child(
                    div()
                        .w(look.px(104.))
                        .flex_shrink_0()
                        .text_color(kit::dim(look))
                        .child(k.to_owned()),
                )
                .child(div().flex_1().min_w_0().child(v))
        };
        let text = |s: String| div().child(s).into_any_element();
        let opened = t.read + t.cited;
        let mut facts: Vec<gpui::AnyElement> = Vec::new();
        facts.push(
            fact(
                "in start blocks",
                text(if t.injected > 0 {
                    format!("{} of {week} sessions this week", t.injected)
                } else {
                    "none this week".into()
                }),
            )
            .into_any_element(),
        );
        if t.injected > 0 {
            facts.push(
                fact(
                    "opened",
                    div()
                        .when(t.mostly_ignored(), |d| d.text_color(look.c.error))
                        .child(format!("{opened} · never opened in {}", t.ignored))
                        .into_any_element(),
                )
                .into_any_element(),
            );
            facts.push(
                fact(
                    "cited in replies",
                    text(format!(
                        "{} time{}",
                        t.cited,
                        if t.cited == 1 { "" } else { "s" }
                    )),
                )
                .into_any_element(),
            );
            facts.push(
                fact(
                    "costs",
                    text(format!(
                        "{} named · {} when opened",
                        human_bytes(t.line_bytes),
                        memory::human(t.bytes)
                    )),
                )
                .into_any_element(),
            );
        } else {
            facts.push(fact("size", text(memory::human(t.bytes))).into_any_element());
        }
        facts.push(
            fact(
                "read any way",
                text(format!(
                    "{} session{} in 30 days{}",
                    t.reads,
                    if t.reads == 1 { "" } else { "s" },
                    if t.edits > 0 {
                        format!(" · {} edits", t.edits)
                    } else {
                        String::new()
                    }
                )),
            )
            .into_any_element(),
        );
        facts.push(
            fact(
                "review",
                match t.overdue {
                    Some(d) => div()
                        .text_color(look.c.warning)
                        .child(format!(
                            "overdue since {} {}",
                            d.day(),
                            &format!("{:?}", d.month())[..3]
                        ))
                        .into_any_element(),
                    None => text(format!("fresh · changed {}", changed_words(t.modified))),
                },
            )
            .into_any_element(),
        );
        let by_profile: Vec<(String, u32)> = stats
            .map(|s| s.by_profile.iter().take(5).cloned().collect())
            .unwrap_or_default();
        let max = by_profile.iter().map(|(_, n)| *n).max().unwrap_or(1).max(1);
        let bars = by_profile.iter().map(|(p, n)| {
            div()
                .flex()
                .items_center()
                .gap(look.px(8.))
                .h(look.px(22.))
                .child(kit::dot(look, kit::profile_hue(p)))
                .child(
                    div()
                        .w(look.px(96.))
                        .truncate()
                        .text_color(look.c.text_muted)
                        .child(kit::profile_name(p)),
                )
                .child(div().flex_1().child(kit::meter(
                    look,
                    *n as f32 / max as f32,
                    look.c.text_muted,
                )))
                .child(
                    div()
                        .w(look.px(28.))
                        .flex()
                        .justify_end()
                        .child(n.to_string()),
                )
        });
        let badge = if t.carried_as == Some(PartKind::Required) || t.required {
            Some("REQUIRED")
        } else if t.carried_as == Some(PartKind::Relevant) {
            Some("RELEVANT")
        } else {
            None
        };
        let path_open = t.path.clone();
        let file = self.workspace_root().map(|r| r.join(&t.path));
        div()
            .id("memory-side")
            .w(look.px(320.))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(look.px(10.))
            .p(look.px(16.))
            .rounded(look.px(8.))
            .border(look.fixed(1.))
            .border_color(look.c.border_variant)
            .bg(look.c.elevated.opacity(0.5))
            .text_size(look.text(12.5))
            .overflow_y_scroll()
            .child(kit::label(
                look,
                &match badge {
                    Some(b) => format!("{} · {b}", t.folder.clone()),
                    None => t.folder.clone(),
                },
                look.c.text_muted,
            ))
            .child(
                div()
                    .text_size(look.text(20.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t.title.clone()),
            )
            .child(div().flex().flex_col().children(facts))
            .when(!by_profile.is_empty(), |d| {
                d.child(
                    div()
                        .pt(look.px(6.))
                        .border_t(look.fixed(1.))
                        .border_color(look.c.border_variant)
                        .child(kit::label(
                            look,
                            "READ BY PROFILE · 30 DAYS",
                            look.c.text_muted,
                        )),
                )
                .children(bars)
            })
            .children(self.memory_tune(t, map, look, cx))
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .gap(look.px(8.))
                    .child(
                        kit::button(look, "memory-open", "Open note ↩", Kind::Primary).on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.open_memory_tile(path_open.clone(), window, cx);
                            }),
                        ),
                    )
                    .children(file.map(|f| {
                        kit::button(look, "memory-editor", "Open in editor ⌘O", Kind::Plain)
                            .on_click(cx.listener(move |_, _, _, cx| cx.open_with_system(&f)))
                    })),
            )
            .into_any_element()
    }

    /// The tune tip for the note, with the x-ray's confirm: the
    /// `required_notes` line before and after, then Change.
    fn memory_tune(
        &self,
        t: &Tile,
        map: &Map,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let tip = map.tip(&t.path)?.clone();
        // A light bulb and the text colour, not a warning callout.
        let mut body = div().flex().flex_col().gap(look.px(8.)).child(
            div()
                .flex()
                .gap(look.px(8.))
                .child(kit::icon(look, "bulb", 15., look.c.text_muted))
                .child(
                    div().flex_1().min_w_0().child(
                        gpui::StyledText::new(format!(
                            "Tune. Never opened in {} of {} sessions. Relevant-only keeps it out of every start block; it is still offered when a task matches it.",
                            tip.ignored, tip.sessions
                        ))
                        .with_highlights(vec![(
                            0..5,
                            gpui::HighlightStyle {
                                font_weight: Some(FontWeight::SEMIBOLD),
                                ..Default::default()
                            },
                        )]),
                    ),
                ),
        );
        match (&self.xray.confirm, tip.action.as_deref()) {
            (Some((c, before, after)), _) if c.note == tip.note => {
                let note = c.note.clone();
                body = body
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(look.px(2.))
                            .p(look.px(8.))
                            .rounded(look.px(6.))
                            .bg(look.c.window.opacity(0.5))
                            .font_family("Menlo")
                            .text_size(look.text(11.))
                            .child(
                                div()
                                    .text_color(look.c.deleted)
                                    .child(format!("- {before}")),
                            )
                            .child(div().text_color(look.c.created).child(format!("+ {after}"))),
                    )
                    .child(
                        div()
                            .text_size(look.text(11.))
                            .text_color(kit::dim(look))
                            .font_family("Menlo")
                            .child(format!("hippocamp xray unrequire {} --yes", note.display())),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(look.px(8.))
                            .child(
                                kit::button(look, "mtune-yes", "Make relevant-only", Kind::Primary)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let args = vec![
                                            "xray".to_owned(),
                                            "unrequire".to_owned(),
                                            note.display().to_string(),
                                            "--yes".to_owned(),
                                        ];
                                        this.flow.log.clear();
                                        this.start_job(Purpose::Tune, vec![args], cx);
                                    })),
                            )
                            .child(
                                kit::button(look, "mtune-no", "Cancel", Kind::Plain).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.xray.confirm = None;
                                        cx.notify();
                                    }),
                                ),
                            ),
                    );
            }
            (_, Some("unrequire")) => {
                let tp = tip.clone();
                body = body.child(
                    div().flex().child(
                        kit::button(look, "mtune-ask", "Make relevant-only…", Kind::Plain)
                            .tooltip(super::tooltip::Tooltip::text(
                                "Shows the change to .hippocamp/hippocamp.toml first; nothing changes until you confirm.",
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.ask_tune(tp.clone(), cx)
                            })),
                    ),
                );
            }
            _ => {}
        }
        if let Some(r) = &self.xray.tune_result {
            let (text, err) = match r {
                Ok(t) => (t.clone(), false),
                Err(e) => (e.clone(), true),
            };
            body = body.child(
                div()
                    .text_size(look.text(11.5))
                    .text_color(if err { look.c.error } else { kit::dim(look) })
                    .child(text),
            );
        }
        Some(
            div()
                .rounded(look.px(8.))
                .border(look.fixed(1.))
                .border_color(look.c.border)
                .bg(look.c.elevated.opacity(0.6))
                .p(look.px(12.))
                .child(body)
                .into_any_element(),
        )
    }

    /// Below 1000 points of pane: a 96-point strip under the map with the
    /// title, its badge, the three counts and Open.
    fn memory_strip(&self, t: &Tile, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let path = t.path.clone();
        let count = |n: String, w: &str| {
            div()
                .flex()
                .flex_col()
                .flex_shrink_0()
                .child(
                    div()
                        .text_size(look.text(15.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(n),
                )
                .child(
                    div()
                        .text_size(look.text(11.))
                        .text_color(kit::dim(look))
                        .child(w.to_owned()),
                )
        };
        div()
            .id("memory-strip")
            .flex()
            .items_center()
            .gap(look.px(16.))
            .h(look.px(72.))
            .flex_shrink_0()
            .px(look.px(14.))
            .rounded(look.px(8.))
            .border(look.fixed(1.))
            .border_color(look.c.border_variant)
            .bg(look.c.elevated.opacity(0.5))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_size(look.text(11.))
                            .text_color(look.c.text_muted)
                            .child(if t.required {
                                format!("{} · required", t.folder)
                            } else {
                                t.folder.clone()
                            }),
                    )
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t.title.clone()),
                    ),
            )
            .child(count(t.injected.to_string(), "start blocks"))
            .child(count(t.ignored.to_string(), "never opened"))
            .child(count(t.reads.to_string(), "reads"))
            .child(
                kit::button(look, "memory-open", "Open note ↩", Kind::Primary).on_click(
                    cx.listener(move |this, _, window, cx| {
                        this.open_memory_tile(path.clone(), window, cx);
                    }),
                ),
            )
            .into_any_element()
    }

    /// The treemap, painted on one canvas, and the hover card over it.
    fn memory_canvas(
        &self,
        map: &Map,
        by: SizeBy,
        filter: &str,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let laid_cell = self.memory_ui.laid.clone();
        let map_p = map.clone();
        let filter_p = filter.to_owned();
        let cost_filter = self.memory_ui.filter;
        let selected = self.memory_shown().map(|t| t.path.clone());
        let hover = self.memory_ui.hover.map(|(i, _)| i);
        let lk = look.clone();
        let font_family = look
            .font_family
            .clone()
            .unwrap_or_else(|| ".SystemUIFont".to_owned());
        let paint = canvas(
            move |bounds, _, _| {
                let area = Rect::new(
                    0.,
                    0.,
                    f32::from(bounds.size.width),
                    f32::from(bounds.size.height),
                );
                let label = f32::from(lk.px(18.));
                let laid = memory::layout(&map_p, by, area, label, f32::from(lk.px(2.)));
                *laid_cell.borrow_mut() = Some((bounds, laid.clone()));
                (laid, lk, map_p, filter_p, selected, hover, font_family)
            },
            move |bounds, (laid, lk, map, filter, selected, hover, family), window, cx| {
                let o = bounds.origin;
                let to_b = |r: &Rect| {
                    Bounds::new(point(o.x + px(r.x), o.y + px(r.y)), size(px(r.w), px(r.h)))
                };
                let mut regular = gpui::font(SharedString::from(family.clone()));
                regular.weight = FontWeight::NORMAL;
                let mut medium = regular.clone();
                medium.weight = FontWeight::MEDIUM;
                let shape = |window: &mut Window,
                             text: &str,
                             font: &gpui::Font,
                             color: Hsla,
                             size_px: Pixels,
                             max: Pixels| {
                    fit_line(window, text, font, color, size_px, max)
                };
                // Group labels.
                for (name, frame, fits) in &laid.groups {
                    if !fits {
                        continue;
                    }
                    let b = to_b(frame);
                    let text = name.to_owned();
                    if let Some(line) = shape(
                        window,
                        &text,
                        &medium,
                        lk.c.text_muted,
                        lk.text(10.5),
                        b.size.width,
                    ) {
                        line.paint(
                            point(b.origin.x + px(2.), b.origin.y + px(1.)),
                            lk.px(14.),
                            gpui::TextAlign::Left,
                            None,
                            window,
                            cx,
                        )
                        .ok();
                    }
                }
                let mut outline: Option<Bounds<Pixels>> = None;
                for (i, r) in &laid.tiles {
                    let t = &map.tiles[*i];
                    let b = to_b(r);
                    if b.size.width < px(1.) || b.size.height < px(1.) {
                        continue;
                    }
                    let lit = matches(t, &filter, cost_filter);
                    let sh = t.shade(by);
                    let mut bg = shade_color(&lk, sh, by);
                    if !lit {
                        bg = bg.opacity(0.18);
                    }
                    if selected.as_ref() == Some(&t.path) {
                        outline = Some(b);
                    }
                    let is_hover = hover == Some(*i);
                    let radius = lk.px(3.).min(b.size.width / 4.);
                    window.paint_quad(quad(
                        b,
                        radius,
                        bg,
                        if is_hover { lk.fixed(1.) } else { px(0.) },
                        lk.c.text_muted,
                        gpui::BorderStyle::Solid,
                    ));
                    // The title and the number, when the tile can hold them.
                    if lit && b.size.width >= lk.px(44.) && b.size.height >= lk.px(30.) {
                        let pad = lk.px(6.);
                        let max = b.size.width - pad * 2.;
                        let fg = on_color(&lk, sh);
                        let mask = gpui::ContentMask { bounds: b };
                        window.with_content_mask(Some(mask), |window| {
                            if let Some(line) =
                                shape(window, &t.title, &medium, fg, lk.text(12.), max)
                            {
                                line.paint(
                                    point(b.origin.x + pad, b.origin.y + lk.px(4.)),
                                    lk.px(16.),
                                    gpui::TextAlign::Left,
                                    None,
                                    window,
                                    cx,
                                )
                                .ok();
                            }
                            if b.size.height >= lk.px(44.)
                                && let Some(line) = shape(
                                    window,
                                    &t.value_label(by),
                                    &regular,
                                    fg.opacity(0.75),
                                    lk.text(11.),
                                    max,
                                )
                            {
                                line.paint(
                                    point(b.origin.x + pad, b.origin.y + lk.px(21.)),
                                    lk.px(14.),
                                    gpui::TextAlign::Left,
                                    None,
                                    window,
                                    cx,
                                )
                                .ok();
                            }
                        });
                    }
                    if t.overdue.is_some()
                        && b.size.width >= lk.px(14.)
                        && b.size.height >= lk.px(14.)
                    {
                        let d = lk.px(7.);
                        let at = point(
                            b.origin.x + b.size.width - d - lk.px(4.),
                            b.origin.y + lk.px(4.),
                        );
                        window.paint_quad(
                            fill(
                                Bounds::new(at, size(d, d)),
                                lk.c.warn.opacity(if lit { 1.0 } else { 0.3 }),
                            )
                            .corner_radii(d / 2.),
                        );
                    }
                }
                // The notes no start block carried: one muted block.
                if let Some((r, n)) = &laid.rest {
                    let b = to_b(r);
                    window.paint_quad(quad(
                        b,
                        lk.px(3.),
                        lk.c.hover.opacity(0.5),
                        lk.fixed(1.),
                        lk.c.border_variant,
                        gpui::BorderStyle::Solid,
                    ));
                    let pad = lk.px(8.);
                    let max = b.size.width - pad * 2.;
                    let mask = gpui::ContentMask { bounds: b };
                    window.with_content_mask(Some(mask), |window| {
                        // Short lines, so a narrow block still says it.
                        let lines = [
                            ("NEVER INJECTED".to_owned(), &medium, lk.text(10.5)),
                            (format!("{n} notes"), &medium, lk.text(12.)),
                            ("switch to Reads".to_owned(), &regular, lk.text(11.5)),
                            ("or Size to see them".to_owned(), &regular, lk.text(11.5)),
                        ];
                        let mut y = b.origin.y + lk.px(6.);
                        for (text, font, sz) in lines {
                            if y + lk.px(14.) > b.origin.y + b.size.height {
                                break;
                            }
                            if let Some(line) = shape(window, &text, font, lk.c.text_muted, sz, max)
                            {
                                line.paint(
                                    point(b.origin.x + pad, y),
                                    lk.px(16.),
                                    gpui::TextAlign::Left,
                                    None,
                                    window,
                                    cx,
                                )
                                .ok();
                            }
                            y += lk.px(17.);
                        }
                    });
                }
                // The selection's outline goes over its neighbours.
                if let Some(b) = outline {
                    window.paint_quad(quad(
                        b,
                        lk.px(3.).min(b.size.width / 4.),
                        gpui::transparent_black(),
                        lk.fixed(2.),
                        lk.c.text,
                        gpui::BorderStyle::Solid,
                    ));
                }
            },
        )
        .size_full();

        let hover_card = self.memory_ui.hover.and_then(|(i, pos)| {
            let t = map.tiles.get(i)?;
            let (bounds, _) = self.memory_ui.laid.borrow().clone()?;
            let w = look.px(260.);
            let mut x = pos.x - bounds.origin.x + look.px(14.);
            if x + w > bounds.size.width {
                x = (pos.x - bounds.origin.x - w - look.px(10.)).max(px(0.));
            }
            let mut y = pos.y - bounds.origin.y + look.px(14.);
            if y + look.px(110.) > bounds.size.height {
                y = (pos.y - bounds.origin.y - look.px(110.)).max(px(0.));
            }
            let mut lines = vec![format!(
                "{} reads · {} · {} edits (30 days)",
                t.reads,
                memory::human(t.bytes),
                t.edits
            )];
            if t.injected > 0 {
                lines.push(format!(
                    "in {} start blocks this week · never opened in {}",
                    t.injected, t.ignored
                ));
            }
            lines.push(format!("changed {}", changed_words(t.modified)));
            if let Some(d) = t.overdue {
                lines.push(format!(
                    "review overdue since {} {:?} {}",
                    d.day(),
                    d.month(),
                    d.year()
                ));
            }
            Some(
                div()
                    .absolute()
                    .left(x)
                    .top(y)
                    .w(w)
                    .p(look.px(10.))
                    .rounded(look.px(6.))
                    .bg(look.c.elevated)
                    .border(look.fixed(1.))
                    .border_color(look.c.border)
                    .shadow_md()
                    .flex()
                    .flex_col()
                    .gap(look.px(3.))
                    .child(
                        div()
                            .text_size(look.text(10.5))
                            .text_color(look.c.text_muted)
                            .child(t.folder.clone()),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_size(look.text(13.))
                            .child(t.title.clone()),
                    )
                    .children(lines.into_iter().map(|l| {
                        div()
                            .text_size(look.text(11.5))
                            .text_color(look.c.text_muted)
                            .child(l)
                    }))
                    .child(
                        div()
                            .text_size(look.text(11.))
                            .text_color(kit::dim(look))
                            .child("click selects · double-click opens"),
                    ),
            )
        });

        let laid_move = self.memory_ui.laid.clone();
        let laid_click = self.memory_ui.laid.clone();
        div()
            .id("memory-map")
            .role(Role::Grid)
            .aria_label("Notes by folder")
            .relative()
            .flex_1()
            .min_h(look.px(120.))
            .rounded(look.px(8.))
            .border(look.fixed(1.))
            .border_color(look.c.border_variant)
            .p(look.px(6.))
            .cursor_pointer()
            .child(paint)
            .on_mouse_move(cx.listener(move |this, e: &gpui::MouseMoveEvent, _, cx| {
                let hit = hit(&laid_move, e.position);
                let before = this.memory_ui.hover.map(|(i, _)| i);
                this.memory_ui.hover = hit.map(|i| (i, e.position));
                if before != hit || hit.is_some() {
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(|this, over: &bool, _, cx| {
                if !*over {
                    this.memory_ui.hover = None;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                if hit_rest(&laid_click, e.position()) {
                    this.set_memory_mode(SizeBy::Reads, cx);
                    return;
                }
                let Some(i) = hit(&laid_click, e.position()) else {
                    return;
                };
                let Some(path) = this
                    .memory_map()
                    .and_then(|m| m.tiles.get(i))
                    .map(|t| t.path.clone())
                else {
                    return;
                };
                window.focus(&this.memory_ui.focus, cx);
                if e.click_count() >= 2 {
                    this.open_memory_tile(path, window, cx);
                } else {
                    this.select_memory_tile(path, cx);
                }
            }))
            .children(hover_card)
            .into_any_element()
    }

    /// The list fallback: one row per note, sorted by the current size.
    fn memory_list(
        &self,
        map: &Map,
        by: SizeBy,
        filter: &str,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let narrow_cols = false;
        let head = |t: &str| {
            div()
                .text_size(look.text(10.5))
                .text_color(look.c.text_muted)
                .child(super::kit::sentence(&t.to_uppercase()))
        };
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        rows.push(
            div()
                .flex()
                .items_center()
                .gap(look.px(10.))
                .px(look.px(10.))
                .h(look.px(26.))
                .child(div().flex_1().min_w_0().child(head("Note")))
                .child(div().w(look.px(110.)).child(head("Folder")))
                .child(div().w(look.px(52.)).child(head("Reads")))
                .child(div().w(look.px(56.)).child(head("Blocks")))
                .child(div().w(look.px(64.)).child(head("Ignored")))
                .child(div().w(look.px(64.)).child(head("Size")))
                .child(div().w(look.px(48.)).child(head("Edits")))
                .child(div().w(look.px(96.)).child(head("Changed")))
                .into_any_element(),
        );
        let _ = narrow_cols;
        for i in map.sorted(by) {
            let t = &map.tiles[i];
            if !matches(t, filter, self.memory_ui.filter) {
                continue;
            }
            let sel = self.memory_ui.selected.as_ref() == Some(&t.path);
            let path = t.path.clone();
            rows.push(
                div()
                    .id(SharedString::from(format!("mrow-{}", t.path.display())))
                    .role(Role::Row)
                    .aria_label(t.title.clone())
                    .flex()
                    .items_center()
                    .gap(look.px(10.))
                    .px(look.px(10.))
                    .h(look.px(30.))
                    .rounded(look.px(5.))
                    .text_size(look.text(12.5))
                    .cursor_pointer()
                    .when(sel, |d| d.bg(look.c.selected))
                    .hover(|h| h.bg(look.c.hover))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(look.px(8.))
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .size(look.px(10.))
                                    .rounded(look.px(2.))
                                    .bg(shade_color(look, t.shade(by), by)),
                            )
                            .child(div().min_w_0().truncate().child(t.title.clone()))
                            .when(t.overdue.is_some(), |d| {
                                d.child(kit::dot(look, look.c.warn))
                            }),
                    )
                    .child(
                        div()
                            .w(look.px(110.))
                            .truncate()
                            .text_color(look.c.text_muted)
                            .child(t.folder.clone()),
                    )
                    .child(div().w(look.px(52.)).child(t.reads.to_string()))
                    .child(div().w(look.px(56.)).child(t.injected.to_string()))
                    .child(
                        div()
                            .w(look.px(64.))
                            .when(t.mostly_ignored(), |d| d.text_color(look.c.error))
                            .child(match t.ignored_share() {
                                Some(s) => format!("{:.0}%", s * 100.0),
                                None => "—".to_owned(),
                            }),
                    )
                    .child(
                        div()
                            .w(look.px(64.))
                            .text_color(look.c.text_muted)
                            .child(memory::human(t.bytes)),
                    )
                    .child(div().w(look.px(48.)).child(t.edits.to_string()))
                    .child(
                        div()
                            .w(look.px(96.))
                            .truncate()
                            .text_color(look.c.text_muted)
                            .child(changed_words(t.modified)),
                    )
                    .on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                        if e.click_count() >= 2 {
                            this.open_memory_tile(path.clone(), window, cx);
                        } else {
                            this.select_memory_tile(path.clone(), cx);
                        }
                    }))
                    .into_any_element(),
            );
        }
        div()
            .id("memory-list")
            .role(Role::Table)
            .aria_label("Notes")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .rounded(look.px(8.))
            .border(look.fixed(1.))
            .border_color(look.c.border_variant)
            .p(look.px(4.))
            .children(rows)
            .into_any_element()
    }
}

/// The tile under `pos` (window coordinates) in the last paint's layout.
fn hit(laid: &LaidCell, pos: Point<Pixels>) -> Option<usize> {
    let cell = laid.borrow();
    let (bounds, laid) = cell.as_ref()?;
    let x = f32::from(pos.x - bounds.origin.x);
    let y = f32::from(pos.y - bounds.origin.y);
    laid.tiles
        .iter()
        .find(|(_, r)| r.contains(x, y))
        .map(|(i, _)| *i)
}

/// `text` shaped in one line, cut with `…` to fit `max`; `None` when not
/// even one character and the ellipsis fit.
fn fit_line(
    window: &mut Window,
    text: &str,
    font: &gpui::Font,
    color: Hsla,
    size_px: Pixels,
    max: Pixels,
) -> Option<gpui::ShapedLine> {
    let run = |len: usize| TextRun {
        len,
        font: font.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let shape = |window: &mut Window, s: String| {
        let len = s.len();
        window
            .text_system()
            .shape_line(SharedString::from(s), size_px, &[run(len)], None)
    };
    let full = shape(window, text.to_owned());
    if full.width() <= max {
        return Some(full);
    }
    // Binary search the longest prefix that fits with the ellipsis.
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let s: String = chars[..mid]
            .iter()
            .collect::<String>()
            .trim_end()
            .to_owned()
            + "…";
        if shape(window, s).width() <= max {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    if lo == 0 {
        return None;
    }
    let s: String = chars[..lo].iter().collect::<String>().trim_end().to_owned() + "…";
    Some(shape(window, s))
}

/// Whether `pos` is on the never-injected block.
fn hit_rest(laid: &LaidCell, pos: Point<Pixels>) -> bool {
    let cell = laid.borrow();
    let Some((bounds, laid)) = cell.as_ref() else {
        return false;
    };
    let x = f32::from(pos.x - bounds.origin.x);
    let y = f32::from(pos.y - bounds.origin.y);
    laid.rest.is_some_and(|(r, _)| r.contains(x, y))
}

impl Shell {
    /// The note page's stat cards (stats first, then the text), each 96
    /// points tall at every width: reads over 30 days with a
    /// sparkline, read by profile, and dreaming as runs and changes.
    pub(super) fn note_stat_cards(
        &self,
        path: &Path,
        page: Option<&hippocamp_view::notepage::NotePage>,
        look: &Look,
    ) -> Option<gpui::AnyElement> {
        let stats = self.note_stats.get(path)?;
        let card = |title: &str| {
            div()
                .flex()
                .flex_col()
                .gap(look.px(4.))
                .flex_1()
                .min_w(look.px(200.))
                .h(look.px(96.))
                .overflow_hidden()
                .p(look.px(12.))
                .rounded(look.px(8.))
                .border(look.fixed(1.))
                .border_color(look.c.border_variant)
                .bg(look.c.elevated.opacity(0.5))
                .child(kit::label(look, title, look.c.text_muted))
        };
        let big = |text: String| {
            div()
                .flex_shrink_0()
                .text_size(look.text(22.))
                .line_height(look.px(26.))
                .font_weight(FontWeight::MEDIUM)
                .child(text)
        };
        let small = |text: String| {
            div()
                .truncate()
                .text_size(look.text(11.5))
                .text_color(look.c.text_muted)
                .child(text)
        };
        let max = stats.per_day.iter().copied().max().unwrap_or(0).max(1);
        let spark = div()
            .flex()
            .items_end()
            .gap(look.px(1.5))
            .h(look.px(26.))
            .flex_1()
            .min_w(look.px(60.))
            .children(stats.per_day.iter().map(|n| {
                let frac = *n as f32 / max as f32;
                div()
                    .flex_1()
                    .max_w(look.px(5.))
                    .h(look.px((2. + 24. * frac).round()))
                    .rounded(look.px(1.))
                    .bg(if *n > 0 {
                        look.c.text_muted
                    } else {
                        look.c.border_variant
                    })
            }));
        let reads = card("Reads · 30 days")
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap(look.px(12.))
                    .child(big(stats.reads.to_string()))
                    .child(spark),
            )
            .child(small(if stats.edits > 0 {
                format!("{} edited it", stats.edits)
            } else {
                "sessions that read, discussed or edited it".into()
            }));
        let by = card("Read by")
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(look.px(6.))
                    .h(look.px(24.))
                    .overflow_hidden()
                    // The top three and `+n`, the side panel's set.
                    .children(stats.by_profile.iter().take(3).map(|(p, n)| {
                        div()
                            .flex()
                            .items_center()
                            .gap(look.px(6.))
                            .h(look.px(22.))
                            .px(look.px(8.))
                            .rounded(look.px(11.))
                            .border(look.fixed(1.))
                            .border_color(look.c.border)
                            .text_size(look.text(11.5))
                            .child(kit::dot(look, kit::profile_hue(p)))
                            // The mark, so three fit; the name is
                            // in the side panel.
                            .child(
                                div()
                                    .text_color(look.c.text_muted)
                                    .child(kit::profile_letter(Some(p))),
                            )
                            .child(div().font_weight(FontWeight::MEDIUM).child(n.to_string()))
                    }))
                    .when(stats.by_profile.len() > 3, |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .h(look.px(22.))
                                .text_size(look.text(11.5))
                                .text_color(look.c.text_muted)
                                .child(format!("+{}", stats.by_profile.len() - 3)),
                        )
                    }),
            )
            .child(small(if stats.by_profile.is_empty() {
                "no session read it in 30 days".into()
            } else if stats.injected > 0 {
                format!(
                    "in {} start blocks · never opened in {}",
                    stats.injected, stats.ignored
                )
            } else {
                format!(
                    "{} profile{}",
                    stats.by_profile.len(),
                    if stats.by_profile.len() == 1 { "" } else { "s" }
                )
            }));
        let runs = page.map_or(&[][..], |p| p.runs.as_slice());
        let changes = page.map_or(0, |p| p.changes);
        // A dry run is listed, marked, and never counted.
        let real = runs.iter().filter(|r| !r.dry_run).count();
        let dreaming = card("Dreaming")
            .gap(look.px(2.))
            .child(
                div()
                    .text_size(look.text(14.))
                    .line_height(look.px(18.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(if real == 0 {
                        "no run looked at it".to_owned()
                    } else {
                        format!(
                            "{} run{} · {} change{}",
                            real,
                            if real == 1 { "" } else { "s" },
                            changes,
                            if changes == 1 { "" } else { "s" }
                        )
                    }),
            )
            .children(runs.iter().take(2).map(|r| {
                div()
                    .flex()
                    .gap(look.px(6.))
                    .h(look.px(15.))
                    .text_size(look.text(11.5))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(look.c.text_muted)
                            .child(r.when.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_color(if r.written > 0 {
                                kit::ok(look)
                            } else {
                                kit::dim(look)
                            })
                            .child(if r.dry_run {
                                format!("dry run · {}", r.outcome)
                            } else {
                                r.outcome.to_owned()
                            }),
                    )
            }));
        Some(
            div()
                .flex()
                .flex_wrap()
                .gap(look.px(10.))
                .pt(look.px(12.))
                .child(reads)
                .child(by)
                .child(dreaming)
                .into_any_element(),
        )
    }
}

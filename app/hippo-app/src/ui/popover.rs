//! Every menu, popover and dropdown in the app opens through here, so each
//! one behaves the same:
//!
//! - it hangs off the control that opened it ([`Hang`]), drawn above
//!   everything else and kept inside the window;
//! - at most one is open ([`Pops::open`]);
//! - it closes on a press outside it, on Escape, on a change of view, and
//!   when the window loses focus;
//! - its rows ([`Shell::pop_row`]) answer ↑ and ↓, and Return picks the
//!   highlighted one, as a click does.
//!
//! A menu is drawn with [`Shell::popover`], the only function that hangs one, and
//! its state is a [`Pop`] in [`Pops`]; there is no other open flag.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use gpui::{AnyElement, Context, Hsla, Window, div, point, prelude::*, px};

use super::Shell;
use super::incidents_ui::Menu;
use super::theme::Look;
use crate::state::Tab;

/// Which popover is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pop {
    /// The Ask bar's "Who answers Ask".
    AskRunner,
    /// A sidebar row's Snooze choices, by row id.
    Snooze(String),
    /// One of Incidents' filter menus.
    IncidentFilter(Menu),
    /// Fleet's account menu.
    Accounts,
    /// Manage's "What the hooks do".
    HooksHelp,
    /// The sidebar foot's health.
    Health,
    /// Brief's profile or repository menu.
    Brief(&'static str),
}

/// What Return on a row does: the same as a click on it.
pub type Pick = Rc<dyn Fn(&mut Shell, &mut Window, &mut Context<Shell>)>;

/// The open popover and its keyboard state.
#[derive(Default)]
pub struct Pops {
    pub open: Option<Pop>,
    /// The view it opened on; another view closes it.
    tab: Option<Tab>,
    /// The highlighted row.
    pub hi: Option<usize>,
    /// The open menu's rows, in order, as it last drew them.
    picks: RefCell<Vec<Pick>>,
    /// The last one a press outside closed, and when: that press's click
    /// on the popover's own trigger does not open it again.
    closed: Option<(Pop, Instant)>,
    /// A popover was built in the last draw.
    drawn: Cell<bool>,
}

impl Pops {
    /// Whether the open popover was built in the last draw.
    #[cfg(test)]
    pub fn drawn(&self) -> bool {
        self.drawn.get()
    }

    /// How many rows the open menu drew.
    #[cfg(test)]
    pub fn rows(&self) -> usize {
        self.picks.borrow().len()
    }
}

/// Where a popover hangs off its trigger: below or above it, lined up
/// with its leading or trailing edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hang {
    BelowStart,
    BelowEnd,
    AboveStart,
    AboveEnd,
}

/// Between a trigger and its popover, in points.
const GAP: f32 = 6.;

impl Shell {
    /// Hangs `body` off the element it is a child of (which must be
    /// `relative()`): the one way to draw a popover. `body` gets the press
    /// outside that closes it and occludes what is under it.
    pub(crate) fn popover(
        &self,
        look: &Look,
        hang: Hang,
        body: gpui::Stateful<gpui::Div>,
        cx: &mut Context<Shell>,
    ) -> AnyElement {
        self.pops.drawn.set(true);
        let (anchor, dy) = match hang {
            Hang::BelowStart => (gpui::Anchor::TopLeft, GAP),
            Hang::BelowEnd => (gpui::Anchor::TopRight, GAP),
            Hang::AboveStart => (gpui::Anchor::BottomLeft, -GAP),
            Hang::AboveEnd => (gpui::Anchor::BottomRight, -GAP),
        };
        // A point at the trigger's corner the popover hangs from.
        let corner = div().absolute().size_0();
        let corner = match hang {
            Hang::BelowStart => corner.bottom_0().left_0(),
            Hang::BelowEnd => corner.bottom_0().right_0(),
            Hang::AboveStart => corner.top_0().left_0(),
            Hang::AboveEnd => corner.top_0().right_0(),
        };
        corner
            .child(
                gpui::deferred(
                    gpui::anchored()
                        .anchor(anchor)
                        .offset(point(px(0.), look.px(dy)))
                        .snap_to_window_with_margin(px(8.))
                        .child(
                            body.occlude()
                                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                    this.close_pop_by_press();
                                    cx.notify();
                                })),
                        ),
                )
                .with_priority(1),
            )
            .into_any_element()
    }

    /// Whether `p` is open.
    pub(crate) fn pop_is(&self, p: &Pop) -> bool {
        self.pops.open.as_ref() == Some(p)
    }

    /// Opens `p`, or closes it when open. A click on the trigger that
    /// follows the press that just closed it leaves it closed.
    pub(crate) fn toggle_pop(&mut self, p: Pop) {
        let just_closed = self
            .pops
            .closed
            .take()
            .is_some_and(|(c, at)| c == p && at.elapsed().as_millis() < 400);
        if self.pop_is(&p) || just_closed {
            self.close_pop();
        } else {
            self.pops.open = Some(p);
            self.pops.tab = self.state.tabs.active_tab().cloned();
            self.pops.hi = None;
            self.pops.picks.borrow_mut().clear();
        }
    }

    /// Closes what is open; whether anything was.
    pub(crate) fn close_pop(&mut self) -> bool {
        self.pops.hi = None;
        self.pops.picks.borrow_mut().clear();
        self.pops.open.take().is_some()
    }

    fn close_pop_by_press(&mut self) {
        if let Some(p) = self.pops.open.clone() {
            self.pops.closed = Some((p, Instant::now()));
        }
        self.close_pop();
    }

    /// Before a draw: a popover opened on another view closes.
    pub(crate) fn sync_pop(&mut self) {
        if self.pops.open.is_some() && self.state.tabs.active_tab() != self.pops.tab.as_ref() {
            self.close_pop();
        }
        // The rows register again as the menu draws.
        self.pops.picks.borrow_mut().clear();
        self.pops.drawn.set(false);
    }

    /// A key while a popover is open: Escape closes it, ↑ and ↓ move
    /// through its rows, Return picks one. Whether the key was used.
    pub(crate) fn pop_key(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.pops.open.is_none() {
            return false;
        }
        let n = self.pops.picks.borrow().len();
        match key {
            "escape" => {
                self.close_pop();
            }
            "down" | "up" if n > 0 => {
                let down = key == "down";
                self.pops.hi = Some(match self.pops.hi {
                    None if down => 0,
                    None => n - 1,
                    Some(i) if down => (i + 1) % n,
                    Some(i) => (i + n - 1) % n,
                });
            }
            "enter" if n > 0 => {
                let Some(i) = self.pops.hi else {
                    return true;
                };
                let pick = self.pops.picks.borrow().get(i).cloned();
                self.close_pop();
                if let Some(pick) = pick {
                    pick(self, window, cx);
                }
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Row `i` of the open menu: a click or Return on it runs `pick` and
    /// closes the menu; highlighted by the keys, it takes `hi_bg`.
    pub(crate) fn pop_row(
        &self,
        i: usize,
        row: gpui::Stateful<gpui::Div>,
        hi_bg: Hsla,
        pick: Pick,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        {
            let mut picks = self.pops.picks.borrow_mut();
            if picks.len() <= i {
                picks.resize(i + 1, Rc::new(|_, _, _| {}));
            }
            picks[i] = pick.clone();
        }
        let lit = self.pops.hi == Some(i);
        row.when(lit, |d| d.bg(hi_bg))
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.close_pop();
                pick(this, window, cx);
                cx.notify();
            }))
    }
}

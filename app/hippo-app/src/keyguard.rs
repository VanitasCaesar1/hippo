//! When a key may write.
//!
//! A script can bring Hippo forward with an Inbox card selected, and a
//! Return typed for another window would then keep a real lesson. A key
//! that writes (Keep, Dismiss, Edit, Save, a confirm dialog's Return, a
//! switch's Space) now acts only when both hold:
//!
//! 1. the window has been active (key and frontmost) for at least
//!    [`SETTLE`], and
//! 2. the view the key acts on has been *engaged* since the window came
//!    forward: the person clicked in it, or moved in it with an arrow key
//!    (or Tab) once the window had settled.
//!
//! Anything else is ignored and the sidebar says why. Clicking a button is
//! never refused: a pointer press is the person's own. The guard is a GPUI
//! global so a control drawn by a free function (a switch) can ask it too.

use std::time::{Duration, Instant};

use crate::state::Tab;

/// How long the window must have been active before a key may write.
pub const SETTLE: Duration = Duration::from_secs(1);

/// Why a keyed write did nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The window is not the active one.
    NotActive,
    /// The window came forward less than [`SETTLE`] ago.
    JustCameForward,
    /// Nothing was clicked or moved in this view since it came forward.
    NotEngaged,
}

impl Refusal {
    /// One line for the sidebar.
    pub fn words(self) -> &'static str {
        match self {
            Refusal::NotActive | Refusal::JustCameForward => {
                "Key ignored: Hippo had only just come forward"
            }
            Refusal::NotEngaged => "Key ignored: click a row or press ↓ first",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct KeyGuard {
    active_since: Option<Instant>,
    engaged: Option<Tab>,
    /// The view on screen, as of the last render.
    shown: Option<Tab>,
    /// The last refusal, for the sidebar, until the person moves.
    pub refused: Option<Refusal>,
}

impl gpui::Global for KeyGuard {}

impl KeyGuard {
    /// The window became active at `now`.
    pub fn activated(&mut self, now: Instant) {
        if self.active_since.is_none() {
            self.active_since = Some(now);
        }
        self.engaged = None;
    }

    /// The window stopped being active: nothing stays engaged.
    pub fn deactivated(&mut self) {
        self.active_since = None;
        self.engaged = None;
    }

    pub fn is_active(&self) -> bool {
        self.active_since.is_some()
    }

    /// Records the view on screen.
    pub fn show(&mut self, tab: Option<Tab>) {
        self.shown = tab;
    }

    pub fn settled(&self, now: Instant) -> bool {
        self.active_since
            .is_some_and(|t| now.saturating_duration_since(t) >= SETTLE)
    }

    /// A real pointer press in the window, on the view shown.
    pub fn clicked(&mut self) {
        if self.active_since.is_some() {
            self.engaged = self.shown.clone();
            self.refused = None;
        }
    }

    /// An arrow key (or Tab) in the view shown. Counts only once the window
    /// has settled: text typed for another window rarely holds arrows, but
    /// the first second is never trusted.
    pub fn navigated(&mut self, now: Instant) {
        if self.settled(now) {
            self.engaged = self.shown.clone();
            self.refused = None;
        }
    }

    /// Says the window came forward just now (tests).
    #[cfg(test)]
    pub fn came_forward_now(&mut self) {
        if self.active_since.is_some() {
            self.active_since = Some(Instant::now());
        }
    }

    /// Moves the time the window came forward back by `by` (tests).
    #[cfg(test)]
    pub fn backdate(&mut self, by: Duration) {
        self.active_since = self.active_since.map(|t| t - by);
    }

    /// Whether a key may write in the view shown, now.
    pub fn check(&self, now: Instant) -> Result<(), Refusal> {
        let Some(since) = self.active_since else {
            return Err(Refusal::NotActive);
        };
        if now.saturating_duration_since(since) < SETTLE {
            return Err(Refusal::JustCameForward);
        }
        match (&self.engaged, &self.shown) {
            (Some(e), Some(s)) if e == s => Ok(()),
            _ => Err(Refusal::NotEngaged),
        }
    }

    /// Whether a page-level Return (onboarding, with no list to engage)
    /// may act: only the settle rule.
    pub fn check_page(&self, now: Instant) -> Result<(), Refusal> {
        match self.active_since {
            None => Err(Refusal::NotActive),
            Some(t) if now.saturating_duration_since(t) < SETTLE => Err(Refusal::JustCameForward),
            Some(_) => Ok(()),
        }
    }

    /// [`check`](Self::check), remembering a refusal for the sidebar.
    pub fn allow(&mut self, now: Instant) -> bool {
        match self.check(now) {
            Ok(()) => true,
            Err(r) => {
                self.refused = Some(r);
                false
            }
        }
    }
}

/// Whether a key may write now, from anywhere that has the app: a refusal
/// is remembered for the sidebar.
pub fn allow(cx: &mut gpui::App) -> bool {
    cx.default_global::<KeyGuard>().allow(Instant::now())
}

/// Whether a key names arrow navigation (or Tab focus moves).
pub fn is_navigation(key: &str) -> bool {
    matches!(key, "up" | "down" | "left" | "right" | "tab")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    fn guard_on(tab: Tab, base: Instant) -> KeyGuard {
        let mut g = KeyGuard::default();
        g.show(Some(tab));
        g.activated(base);
        g
    }

    #[test]
    fn a_key_right_after_coming_forward_does_nothing() {
        let t = Instant::now();
        let mut g = guard_on(Tab::Inbox, t);
        // A script brings the app forward with a card selected, a stray Return.
        assert_eq!(g.check(at(t, 200)), Err(Refusal::JustCameForward));
        assert!(!g.allow(at(t, 200)));
        assert_eq!(g.refused, Some(Refusal::JustCameForward));
        // Even a click inside the first second waits for the second to pass.
        g.clicked();
        assert_eq!(g.check(at(t, 500)), Err(Refusal::JustCameForward));
        assert_eq!(g.check(at(t, 1000)), Ok(()));
    }

    #[test]
    fn a_settled_window_still_needs_a_click_or_an_arrow() {
        let t = Instant::now();
        let mut g = guard_on(Tab::Inbox, t);
        assert_eq!(g.check(at(t, 5000)), Err(Refusal::NotEngaged));
        // An arrow inside the first second does not engage...
        g.navigated(at(t, 400));
        assert_eq!(g.check(at(t, 5000)), Err(Refusal::NotEngaged));
        // ...one after it does.
        g.navigated(at(t, 1200));
        assert_eq!(g.check(at(t, 1300)), Ok(()));
        assert_eq!(g.refused, None);
    }

    #[test]
    fn engagement_belongs_to_one_view_and_ends_when_the_window_leaves() {
        let t = Instant::now();
        let mut g = guard_on(Tab::Fleet, t);
        g.clicked();
        assert_eq!(g.check(at(t, 2000)), Ok(()));
        // ⌘2 to the Inbox: the Fleet click does not carry over.
        g.show(Some(Tab::Inbox));
        assert_eq!(g.check(at(t, 2000)), Err(Refusal::NotEngaged));
        g.clicked();
        assert_eq!(g.check(at(t, 2000)), Ok(()));
        // Away and back: nothing is engaged, and the second starts again.
        g.deactivated();
        assert_eq!(g.check(at(t, 3000)), Err(Refusal::NotActive));
        g.activated(at(t, 4000));
        assert_eq!(g.check(at(t, 4500)), Err(Refusal::JustCameForward));
        assert_eq!(g.check(at(t, 6000)), Err(Refusal::NotEngaged));
    }

    #[test]
    fn a_click_while_inactive_does_not_engage() {
        let t = Instant::now();
        let mut g = KeyGuard::default();
        g.show(Some(Tab::Inbox));
        g.clicked();
        g.activated(t);
        assert_eq!(g.check(at(t, 2000)), Err(Refusal::NotEngaged));
    }

    #[test]
    fn page_returns_need_only_the_settle_rule() {
        let t = Instant::now();
        let g = guard_on(Tab::Home, t);
        assert_eq!(g.check_page(at(t, 100)), Err(Refusal::JustCameForward));
        assert_eq!(g.check_page(at(t, 1100)), Ok(()));
        assert_eq!(KeyGuard::default().check_page(t), Err(Refusal::NotActive));
    }

    #[test]
    fn navigation_keys() {
        for k in ["up", "down", "left", "right", "tab"] {
            assert!(is_navigation(k));
        }
        for k in ["enter", "j", "k", "e", "backspace", "space"] {
            assert!(!is_navigation(k));
        }
    }
}

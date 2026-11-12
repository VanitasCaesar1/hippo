//! A thin vertical scrollbar drawn over a scrolled element. GPUI draws
//! none; without one a page that scrolls looks cut off at the bottom.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{AnimationExt as _, IntoElement, ScrollHandle, div, prelude::*, px};

use super::theme::Look;

/// The thumb's top and height, in pixels, for a viewport of `viewport`
/// pixels scrolled `scrolled` of `max` pixels; `None` when nothing
/// scrolls.
pub fn thumb(viewport: f32, max: f32, scrolled: f32, min: f32) -> Option<(f32, f32)> {
    if max <= 0.5 || viewport <= 0. {
        return None;
    }
    let height = (viewport * viewport / (viewport + max))
        .max(min)
        .min(viewport);
    let top = (scrolled / max).clamp(0., 1.) * (viewport - height);
    Some((top, height))
}

/// The scrollbar of the element `handle` tracks, to place over it in a
/// `relative` box of the same size; `None` when it does not scroll (or has
/// not been drawn yet).
pub fn vertical(handle: &ScrollHandle, look: &Look) -> Option<impl IntoElement> {
    let viewport = f32::from(handle.bounds().size.height);
    let max = f32::from(handle.max_offset().y);
    let scrolled = -f32::from(handle.offset().y);
    let (top, height) = thumb(viewport, max, scrolled, f32::from(look.px(24.)))?;
    // An overlay scrollbar, as macOS draws them: it shows while
    // the element scrolls and fades out after it stops.
    let origin = handle.bounds().origin;
    let key = (f32::from(origin.x) as i32, f32::from(origin.y) as i32);
    let since = moved(key, scrolled)?;
    Some(
        div()
            .absolute()
            .top(px(top))
            .right(look.px(2.))
            .w(look.px(6.))
            .h(px(height))
            .rounded(look.px(3.))
            .bg(look.c.scrollbar_thumb)
            .with_animation(
                gpui::ElementId::Name(format!("scrollbar-{}-{}-{since}", key.0, key.1).into()),
                gpui::Animation::new(SHOWN + FADE),
                |d, t| {
                    let shown = SHOWN.as_secs_f32() / (SHOWN + FADE).as_secs_f32();
                    d.opacity(if t < shown {
                        1.
                    } else {
                        (1. - t) / (1. - shown)
                    })
                },
            ),
    )
}

/// How long the thumb stays after the last scroll, then how long it fades.
const SHOWN: Duration = Duration::from_millis(1000);
const FADE: Duration = Duration::from_millis(300);

/// A scrolled element's last offset, when it changed, and how many times.
type Seen = (f32, Instant, u64);

thread_local! {
    /// Each scrolled element's [`Seen`], by place.
    static SEEN: RefCell<HashMap<(i32, i32), Seen>> = RefCell::new(HashMap::new());
}

/// When the element at `key` last scrolled (a number that changes with
/// each new scroll), or `None` once its thumb has faded.
fn moved(key: (i32, i32), scrolled: f32) -> Option<u64> {
    let now = Instant::now();
    SEEN.with(|m| {
        let mut m = m.borrow_mut();
        let e = m
            .entry(key)
            .or_insert((scrolled, now - SHOWN - FADE - FADE, 0));
        if (e.0 - scrolled).abs() > 0.5 {
            *e = (scrolled, now, e.2 + 1);
        }
        (now.duration_since(e.1) < SHOWN + FADE).then_some(e.2)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_thumb_shows_how_much_is_seen_and_where() {
        assert_eq!(thumb(600., 0., 0., 24.), None, "nothing to scroll");
        // A page twice the viewport: half the bar, at the top, then the end.
        assert_eq!(thumb(600., 600., 0., 24.), Some((0., 300.)));
        assert_eq!(thumb(600., 600., 600., 24.), Some((300., 300.)));
        // A very long page keeps a thumb one can see.
        assert_eq!(thumb(600., 600_000., 0., 24.).unwrap().1, 24.);
    }
}

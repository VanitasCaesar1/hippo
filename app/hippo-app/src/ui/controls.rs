//! Painted controls for Manage: a switch, a stepper and a slider.
//! Each
//! is drawn with GPUI elements and reachable from the keyboard; the caller
//! attaches what a change does (Manage opens its confirmation sheet).

use std::cell::Cell;
use std::rc::Rc;

use gpui::{App, Bounds, ClickEvent, ElementId, Pixels, Role, Window, canvas, div, prelude::*, px};

use super::theme::Look;

/// What a control does when used: a click, Space or Return.
pub type Act = Rc<dyn Fn(&mut Window, &mut App)>;

/// A switch, 36 by 20, the working green when on (amber is for
/// the primary action and selection only). A click, Space or Return runs
/// `act`; `enabled` false dims it and drops both.
pub fn switch(
    look: &Look,
    id: impl Into<ElementId>,
    label: impl Into<String>,
    on: bool,
    enabled: bool,
    act: Act,
) -> gpui::Stateful<gpui::Div> {
    let label = label.into();
    // On is the accent (blue), not the working green.
    let track = if on { look.c.accent } else { look.c.border };
    let d = div()
        .id(id.into())
        .role(Role::Switch)
        .aria_label(label)
        .aria_selected(on)
        .tab_stop(enabled)
        .flex_shrink_0()
        .w(look.px(36.))
        .h(look.px(20.))
        .p(look.px(2.))
        .rounded(look.px(10.))
        .bg(track)
        .flex()
        .items_center()
        .when(on, |d| d.justify_end())
        .focus_visible(|s| s.border_2().border_color(look.c.focus))
        .child(div().size(look.px(16.)).rounded_full().bg(if on {
            look.c.window
        } else {
            look.c.text_muted
        }));
    if enabled {
        let key = act.clone();
        d.cursor_pointer()
            .hover(|h| h.opacity(0.88))
            .on_click(move |e: &ClickEvent, window, cx| {
                // Space or Return on a focused switch passes the key guard.
                if e.is_keyboard() && !crate::keyguard::allow(cx) {
                    return;
                }
                act(window, cx)
            })
            .on_key_down(move |e: &gpui::KeyDownEvent, window, cx| {
                if matches!(e.keystroke.key.as_str(), "space" | "enter") {
                    if crate::keyguard::allow(cx) {
                        key(window, cx);
                    }
                    cx.stop_propagation();
                }
            })
    } else {
        d.opacity(0.45)
    }
}

fn step_button(
    look: &Look,
    id: ElementId,
    glyph: &'static str,
    enabled: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(if glyph == "−" { "Less" } else { "More" })
        .flex_shrink_0()
        .w(look.px(26.))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .text_size(look.text(14.))
        .text_color(if enabled {
            look.c.text_muted
        } else {
            look.c.text_muted.opacity(0.4)
        })
        .when(enabled, |d| {
            d.cursor_pointer()
                .hover(|h| h.bg(look.c.hover).text_color(look.c.text))
        })
        .child(glyph)
}

/// A stepper: − value +. `minus` and `plus` run on the buttons' clicks.
#[allow(clippy::too_many_arguments)]
pub fn stepper(
    look: &Look,
    id: &str,
    value: impl Into<String>,
    width: f32,
    enabled: bool,
    minus: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    plus: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Div {
    let m = step_button(
        look,
        ElementId::Name(format!("{id}-minus").into()),
        "−",
        enabled,
    );
    let p = step_button(
        look,
        ElementId::Name(format!("{id}-plus").into()),
        "+",
        enabled,
    );
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .h(look.px(28.))
        .rounded(look.px(6.))
        .border(look.fixed(1.))
        .border_color(look.c.border)
        .overflow_hidden()
        .child(if enabled { m.on_click(minus) } else { m })
        .child(
            div()
                .w(look.px(width))
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .border_l(look.fixed(1.))
                .border_r(look.fixed(1.))
                .border_color(look.c.border_variant)
                .text_size(look.text(12.5))
                .child(value.into()),
        )
        .child(if enabled { p.on_click(plus) } else { p })
}

/// A slider's track bounds, kept by its paint for the pointer handlers.
pub type TrackBounds = Rc<Cell<Option<Bounds<Pixels>>>>;

/// The fraction (0 to 1) of the track at window x `x`.
pub fn track_fraction(track: &TrackBounds, x: Pixels) -> Option<f32> {
    let b = track.get()?;
    if b.size.width <= px(0.) {
        return None;
    }
    Some((f32::from(x - b.origin.x) / f32::from(b.size.width)).clamp(0., 1.))
}

/// A slider: a track, its filled part in the accent and a round thumb at
/// `frac`. The caller handles the pointer on the returned element (down,
/// move with the button held, up) with [`track_fraction`].
pub fn slider(
    look: &Look,
    id: impl Into<ElementId>,
    frac: f32,
    track: TrackBounds,
) -> gpui::Stateful<gpui::Div> {
    let frac = frac.clamp(0., 1.);
    let thumb = look.px(18.);
    let measure = {
        let track = track.clone();
        canvas(move |bounds, _, _| track.set(Some(bounds)), |_, _, _, _| {})
            .absolute()
            .top_0()
            .left_0()
            .size_full()
    };
    div()
        .id(id.into())
        .role(Role::Slider)
        .tab_stop(true)
        .relative()
        .flex_1()
        .min_w(look.px(120.))
        .h(thumb)
        .cursor_pointer()
        .child(measure)
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(look.px(7.))
                .h(look.px(4.))
                .rounded(look.px(2.))
                .bg(look.c.border),
        )
        .child(
            div()
                .absolute()
                .left_0()
                .top(look.px(7.))
                .h(look.px(4.))
                .w(gpui::relative(frac))
                .rounded(look.px(2.))
                // The text colour; the accent is for the one action.
                .bg(look.c.text_muted),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .left(gpui::relative(frac))
                .ml(-thumb / 2.)
                .size(thumb)
                .rounded_full()
                .bg(look.c.text)
                .border(look.fixed(2.))
                .border_color(look.c.window),
        )
}

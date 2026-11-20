//! The night map's tokens and parts (`docs/designs/app-night-map.md`):
//! colours with four meanings, cards, chips, counts, the controls
//! (buttons, icon buttons, keys, a select, a menu, segments), agent
//! marks, quotes, the dotted ground, the right-angled connectors and the
//! strip.

use gpui::{
    AnyElement, FontWeight, Hsla, PathBuilder, Pixels, Point, Role, canvas, div, point, prelude::*,
    px, rgba, svg,
};
use hippo_view::night::Agent;

use crate::ui::theme::Look;

pub use crate::ui::kit::MONO;

/// The tokens.
#[derive(Clone, Copy)]
pub struct Nt {
    pub canvas: Hsla,
    pub dot: Hsla,
    pub surface: Hsla,
    pub raised: Hsla,
    pub inset: Hsla,
    pub hover: Hsla,
    pub selected: Hsla,
    pub line: Hsla,
    pub line2: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub faint: Hsla,
    pub you: Hsla,
    pub you_ink: Hsla,
    pub you_text: Hsla,
    pub you_bg: Hsla,
    pub you_line: Hsla,
    pub night: Hsla,
    pub night_bg: Hsla,
    pub night_line: Hsla,
    pub ok: Hsla,
    pub ok_bg: Hsla,
    pub bad: Hsla,
    pub bad_ink: Hsla,
    pub bad_bg: Hsla,
    pub shadow: Hsla,
    pub dark: bool,
}

fn c(hex: u32) -> Hsla {
    rgba((hex << 8) | 0xFF).into()
}

fn ca(hex: u32, a: f32) -> Hsla {
    let h: Hsla = rgba((hex << 8) | 0xFF).into();
    h.opacity(a)
}

impl Nt {
    /// The night map's own values in Hippo's themes; in another theme its
    /// surfaces and text, with the four meanings kept.
    pub fn of(look: &Look) -> Nt {
        let dark = look.c.dark;
        let mut t = if dark {
            Nt {
                canvas: c(0x0B0D14),
                dot: ca(0xA0AFE1, 0.15),
                surface: c(0x12151F),
                raised: c(0x191D2B),
                inset: c(0x0D1018),
                hover: c(0x1C2131),
                selected: c(0x20263C),
                line: ca(0xA0AFE1, 0.13),
                line2: ca(0xA0AFE1, 0.26),
                text: c(0xECEEF6),
                muted: c(0xA7AEC6),
                faint: c(0x7F87A3),
                you: c(0xFFC53D),
                you_ink: c(0x1C1400),
                you_text: c(0xFFD778),
                you_bg: ca(0xFFC53D, 0.13),
                you_line: ca(0xFFC53D, 0.5),
                night: c(0x96A1FF),
                night_bg: ca(0x96A1FF, 0.13),
                night_line: ca(0x96A1FF, 0.55),
                ok: c(0x45D992),
                ok_bg: ca(0x45D992, 0.12),
                bad: c(0xFF7A6B),
                bad_ink: c(0x2A0600),
                bad_bg: ca(0xFF7A6B, 0.12),
                shadow: ca(0x000000, 0.42),
                dark,
            }
        } else {
            Nt {
                canvas: c(0xF3F4F8),
                dot: ca(0x1B2440, 0.15),
                surface: c(0xFFFFFF),
                raised: c(0xFFFFFF),
                inset: c(0xEDEFF5),
                hover: c(0xF0F2F8),
                selected: c(0xE7EAF8),
                line: ca(0x1B2440, 0.11),
                line2: ca(0x1B2440, 0.24),
                text: c(0x161D33),
                muted: c(0x4A5370),
                faint: c(0x5D6684),
                you: c(0xF7BC1C),
                you_ink: c(0x1C1400),
                you_text: c(0x7A5200),
                you_bg: c(0xFFEDB3),
                you_line: c(0xD9A514),
                night: c(0x4352C7),
                night_bg: ca(0x4352C7, 0.09),
                night_line: ca(0x4352C7, 0.5),
                ok: c(0x0E6B40),
                ok_bg: ca(0x0E6B40, 0.1),
                bad: c(0xB3261E),
                bad_ink: c(0xFFFFFF),
                bad_bg: ca(0xB3261E, 0.08),
                shadow: ca(0x1B2440, 0.13),
                dark,
            }
        };
        if !look.hippo_theme {
            t.canvas = look.c.window;
            t.surface = look.c.elevated;
            t.raised = look.c.selected;
            t.inset = look.c.dock;
            t.hover = look.c.hover;
            t.selected = look.c.selected;
            t.line = look.c.border;
            t.line2 = look.c.border_variant;
            t.text = look.c.text;
            t.muted = look.c.text_muted;
            t.faint = look.c.text_placeholder;
            t.dot = look.c.text.opacity(0.12);
        }
        t
    }
}

/// An icon from the app's assets.
pub fn icon(look: &Look, name: &str, size: f32, color: Hsla) -> gpui::Svg {
    svg()
        .path(format!("icons/n-{name}.svg"))
        .flex_shrink_0()
        .size(look.px(size))
        .text_color(color)
}

/// A card: surface, a 1-point border, 14-point corners, 14 points in.
pub fn card(look: &Look, t: &Nt) -> gpui::Div {
    div()
        .bg(t.surface)
        .border(look.fixed(1.))
        .border_color(t.line)
        .rounded(look.px(14.))
        .p(look.px(14.))
        .shadow(vec![gpui::BoxShadow {
            color: t.shadow.opacity(if t.dark { 0.6 } else { 0.8 }),
            offset: point(px(0.), look.px(1.)),
            blur_radius: look.px(3.),
            spread_radius: px(0.),
            inset: false,
        }])
}

/// A selectable card: its 2-point border is always there, indigo when
/// selected, so nothing moves.
pub fn select_card(look: &Look, t: &Nt, selected: bool) -> gpui::Div {
    card(look, t)
        .border(look.px(2.))
        .border_color(if selected { t.night } else { t.line })
        .p(look.px(13.))
}

/// A floating thing's shadow: the panel, the Ask bar.
pub fn float_shadow(look: &Look, t: &Nt) -> Vec<gpui::BoxShadow> {
    vec![gpui::BoxShadow {
        color: t.shadow,
        offset: point(px(0.), look.px(10.)),
        blur_radius: look.px(28.),
        spread_radius: px(0.),
        inset: false,
    }]
}

/// A 26-point icon tile, tinted by `tone`.
pub fn tile(look: &Look, t: &Nt, name: &str, tone: Tone) -> gpui::Div {
    let (bg, fg) = tone.colors(t);
    div()
        .size(look.px(26.))
        .flex_shrink_0()
        .rounded(look.px(8.))
        .bg(bg)
        .flex()
        .items_center()
        .justify_center()
        .child(icon(look, name, 16., fg))
}

/// What a chip, tile or count means.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    Plain,
    You,
    Night,
    Ok,
    Bad,
}

impl Tone {
    /// The tone a model's word names (`ok`, `you`, `bad`, `night`).
    pub fn of(word: &str) -> Tone {
        match word {
            "you" => Tone::You,
            "night" => Tone::Night,
            "ok" => Tone::Ok,
            "bad" => Tone::Bad,
            _ => Tone::Plain,
        }
    }

    /// Ground and ink.
    pub fn colors(self, t: &Nt) -> (Hsla, Hsla) {
        match self {
            Tone::Plain => (t.inset, t.muted),
            Tone::You => (t.you_bg, t.you_text),
            Tone::Night => (t.night_bg, t.night),
            Tone::Ok => (t.ok_bg, t.ok),
            Tone::Bad => (t.bad_bg, t.bad),
        }
    }
}

/// A 20-point chip; tinted ones lead with a dot.
pub fn chip(look: &Look, t: &Nt, text: impl Into<String>, tone: Tone) -> gpui::Div {
    let (bg, fg) = tone.colors(t);
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(look.px(5.))
        .h(look.px(20.))
        .px(look.px(7.))
        .rounded(look.px(6.))
        .bg(bg)
        .text_color(fg)
        .text_size(look.text(11.5))
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .when(matches!(tone, Tone::You | Tone::Ok | Tone::Bad), |d| {
            d.child(div().size(look.px(6.)).rounded_full().bg(fg))
        })
        .child(text.into())
}

/// A round count: amber waits on you, red is guard, grey is plain.
pub fn count(look: &Look, t: &Nt, n: usize, tone: Tone) -> gpui::Div {
    let (bg, fg) = match tone {
        Tone::You => (t.you, t.you_ink),
        Tone::Bad => (t.bad, t.bad_ink),
        _ => (t.inset, t.muted),
    };
    div()
        .flex_shrink_0()
        .min_w(look.px(18.))
        .h(look.px(18.))
        .px(look.px(5.))
        .rounded_full()
        .bg(bg)
        .text_color(fg)
        .text_size(look.text(11.))
        .font_weight(FontWeight::BOLD)
        .flex()
        .items_center()
        .justify_center()
        .child(n.to_string())
}

/// The file of an agent's mark (brand/agents/README.md), or Lucide's
/// terminal for an agent with none.
pub fn mark_path(a: &Agent) -> &'static str {
    match a.code.as_str() {
        "cc" => "icons/agent-claude-code.svg",
        "cx" => "icons/agent-codex.svg",
        "cu" => "icons/agent-cursor.svg",
        "gm" => "icons/agent-gemini-cli.svg",
        "oc" => "icons/agent-opencode.svg",
        "ds" => "icons/agent-deepseek.svg",
        "mu" => "icons/agent-muse.svg",
        "co" => "icons/agent-command-code.svg",
        _ => "icons/terminal.svg",
    }
}

/// An agent's own mark, one colour (`color`, normally the text's), at
/// `size` points (14, 16 or 20). It says which agent; the agent's name is
/// next to it or in its accessible label.
pub fn mark(look: &Look, a: &Agent, size: f32, color: Hsla) -> gpui::Svg {
    svg()
        .path(mark_path(a))
        .flex_shrink_0()
        .size(look.px(size))
        .text_color(color)
}

/// The mark in the muted text colour, at 16 points: what replaces the old
/// two-letter badge in rows and cards.
pub fn badge(look: &Look, t: &Nt, a: &Agent) -> gpui::Svg {
    mark(look, a, 16., t.muted)
}

/// What a button is.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Btn {
    /// Amber: at most one per card.
    Go,
    /// Bordered.
    Second,
    /// No border.
    Quiet,
}

/// A control's height: 28, 32 or 36 points.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Size {
    Sm,
    Md,
    Lg,
}

impl Size {
    pub fn h(self) -> f32 {
        match self {
            Size::Sm => 28.,
            Size::Md => 32.,
            Size::Lg => 36.,
        }
    }
    fn pad(self) -> f32 {
        match self {
            Size::Sm => 10.,
            Size::Md => 12.,
            Size::Lg => 14.,
        }
    }
    fn icon(self) -> f32 {
        match self {
            Size::Lg => 16.,
            _ => 14.,
        }
    }
}

/// The states every control shares: hover, pressed, the keyboard focus
/// ring, a place in the tab order, and the pointer.
fn interactive(
    d: gpui::Stateful<gpui::Div>,
    look: &Look,
    t: &Nt,
    kind: Btn,
) -> gpui::Stateful<gpui::Div> {
    let (hover, press, ring) = (t.hover, t.selected, t.night);
    let w = look.px(2.);
    d.tab_index(0)
        .cursor_pointer()
        .focus_visible(move |s| s.border_color(ring).border(w))
        .when(kind == Btn::Go, |d| {
            d.hover(|h| h.opacity(0.92)).active(|a| a.opacity(0.8))
        })
        .when(kind != Btn::Go, move |d| {
            d.hover(move |h| h.bg(hover)).active(move |a| a.bg(press))
        })
}

/// A button: a label with an optional Lucide icon before it. `small` is
/// 28 points high, otherwise 32.
pub fn button(
    look: &Look,
    t: &Nt,
    id: impl Into<gpui::ElementId>,
    lead: Option<&str>,
    label: impl Into<String>,
    kind: Btn,
    small: bool,
) -> gpui::Stateful<gpui::Div> {
    button_sized(
        look,
        t,
        id,
        lead,
        label,
        kind,
        if small { Size::Sm } else { Size::Md },
    )
}

/// [`button`] at a given [`Size`].
pub fn button_sized(
    look: &Look,
    t: &Nt,
    id: impl Into<gpui::ElementId>,
    lead: Option<&str>,
    label: impl Into<String>,
    kind: Btn,
    size: Size,
) -> gpui::Stateful<gpui::Div> {
    let label = label.into();
    let (bg, fg, border) = match kind {
        Btn::Go => (t.you, t.you_ink, t.you),
        Btn::Second => (t.raised, t.text, t.line2),
        Btn::Quiet => (gpui::transparent_black(), t.text, gpui::transparent_black()),
    };
    let d = div()
        .id(id.into())
        .role(Role::Button)
        .aria_label(label.clone())
        .flex_shrink_0()
        .h(look.px(size.h()))
        .px(look.px(size.pad()))
        .flex()
        .items_center()
        .gap(look.px(6.))
        .rounded(look.px(8.))
        .bg(bg)
        .border(look.fixed(1.))
        .border_color(border)
        .text_color(fg)
        .text_size(look.text(13.))
        .font_weight(FontWeight::SEMIBOLD)
        .whitespace_nowrap()
        .children(lead.map(|n| icon(look, n, size.icon(), fg)))
        .child(label);
    interactive(d, look, t, kind)
}

/// A button drawn as a disabled one: dimmed, no pointer, no hover; the
/// caller attaches no click.
pub fn disabled(d: gpui::Stateful<gpui::Div>) -> gpui::Stateful<gpui::Div> {
    d.opacity(0.45).cursor_default()
}

/// A square button with one Lucide icon; `label` is its accessible name
/// and its tooltip, with `keys` (a shortcut such as `⌘=`) beside it.
pub fn icon_button(
    look: &Look,
    t: &Nt,
    id: impl Into<gpui::ElementId>,
    name: &str,
    label: &str,
    keys: Option<&str>,
    size: Size,
) -> gpui::Stateful<gpui::Div> {
    let tip = crate::ui::tooltip::Tooltip::with_keys(label, keys);
    let d = div()
        .id(id.into())
        .role(Role::Button)
        .aria_label(label.to_owned())
        .flex_shrink_0()
        .size(look.px(size.h()))
        .flex()
        .items_center()
        .justify_center()
        .rounded(look.px(8.))
        .border(look.fixed(1.))
        .border_color(gpui::transparent_black())
        .text_color(t.muted)
        .child(icon(look, name, size.icon() + 2., t.muted))
        .tooltip(tip);
    interactive(d, look, t, Btn::Quiet)
}

/// One key of a shortcut: a Lucide icon for ⌘ ⌥ ⇧ ↩ ⌫ and the arrows,
/// the letter otherwise, in a small bordered box.
pub fn kbd(look: &Look, t: &Nt, keys: &str) -> gpui::Div {
    let mut row = div().flex().flex_shrink_0().items_center().gap(look.px(2.));
    for k in keys.chars() {
        let key = div()
            .min_w(look.px(18.))
            .h(look.px(18.))
            .px(look.px(4.))
            .rounded(look.px(4.))
            .border(look.fixed(1.))
            .border_color(t.line2)
            .bg(t.inset)
            .flex()
            .items_center()
            .justify_center()
            .text_size(look.text(11.))
            .text_color(t.muted);
        let lucide = match k {
            '⌘' => Some("command"),
            '⌥' => Some("option"),
            '⇧' => Some("arrow-big-up"),
            '↩' | '⏎' => Some("corner-down-left"),
            '⌫' => Some("delete"),
            '↑' => Some("arrow-up"),
            '↓' => Some("arrow-down"),
            '←' => Some("arrow-left"),
            '→' => Some("arrow-right"),
            _ => None,
        };
        row = row.child(match lucide {
            Some(n) => key.child(icon_plain(look, n, 11., t.muted)),
            None => key.child(k.to_string()),
        });
    }
    row
}

/// A Lucide icon by its own name.
pub fn icon_plain(look: &Look, name: &str, size: f32, color: Hsla) -> gpui::Svg {
    svg()
        .path(format!("icons/{name}.svg"))
        .flex_shrink_0()
        .size(look.px(size))
        .text_color(color)
}

/// A compact select's trigger: what is chosen (with `lead` before it, such
/// as an agent's mark) and the up-down chevrons; the caller opens the menu.
pub fn select(
    look: &Look,
    t: &Nt,
    id: impl Into<gpui::ElementId>,
    lead: Option<AnyElement>,
    label: impl Into<String>,
    aria: &str,
) -> gpui::Stateful<gpui::Div> {
    let d = div()
        .id(id.into())
        .role(Role::Button)
        .aria_label(aria.to_owned())
        .flex_shrink_0()
        .h(look.px(Size::Sm.h()))
        .pl(look.px(8.))
        .pr(look.px(6.))
        .flex()
        .items_center()
        .gap(look.px(6.))
        .rounded(look.px(8.))
        .border(look.fixed(1.))
        .border_color(t.line)
        .bg(t.raised)
        .text_size(look.text(13.))
        .text_color(t.text)
        .whitespace_nowrap()
        .children(lead)
        .child(label.into())
        .child(icon_plain(look, "chevrons-up-down", 13., t.faint));
    interactive(d, look, t, Btn::Second)
}

/// A menu: a floating list of rows, each a Lucide icon or mark, a label
/// and a check on the chosen one. The caller places it and attaches
/// clicks to the rows it returns, in order.
pub fn menu_row(
    look: &Look,
    t: &Nt,
    id: impl Into<gpui::ElementId>,
    lead: Option<AnyElement>,
    label: impl Into<String>,
    chosen: bool,
) -> gpui::Stateful<gpui::Div> {
    let d = div()
        .id(id.into())
        .role(Role::MenuItem)
        .h(look.px(Size::Md.h()))
        .px(look.px(10.))
        .flex()
        .items_center()
        .gap(look.px(8.))
        .rounded(look.px(6.))
        .border(look.fixed(1.))
        .border_color(gpui::transparent_black())
        .text_size(look.text(13.))
        .text_color(t.text)
        .children(lead)
        .child(div().flex_1().min_w_0().truncate().child(label.into()))
        .when(chosen, |d| d.child(icon_plain(look, "check", 14., t.night)));
    interactive(d, look, t, Btn::Quiet)
}

/// The menu's surface.
pub fn menu(look: &Look, t: &Nt) -> gpui::Div {
    div()
        .min_w(look.px(220.))
        .p(look.px(4.))
        .flex()
        .flex_col()
        .gap(look.px(2.))
        .rounded(look.px(10.))
        .bg(t.surface)
        .border(look.fixed(1.))
        .border_color(t.line2)
        .shadow(float_shadow(look, t))
}

/// Your own words on the amber highlight, the whole quote one rounded
/// block (GPUI's text runs have square, unpadded backgrounds).
pub fn said(look: &Look, t: &Nt, text: &str, lines: usize) -> gpui::Div {
    div()
        .flex_1()
        .min_w_0()
        .bg(t.you_bg)
        .rounded(look.px(4.))
        .pl(look.px(5.))
        .pr(look.px(12.))
        .text_color(t.you_text)
        .text_size(look.text(13.))
        .line_height(look.text(19.))
        .line_clamp(lines)
        .child(text.to_owned())
}

/// A quote with a 2-point bar at its left.
pub fn quote(look: &Look, bar: Hsla, color: Hsla, text: &str, size: f32) -> gpui::Div {
    div()
        .flex()
        .gap(look.px(10.))
        .child(div().w(look.px(2.)).flex_shrink_0().rounded_full().bg(bar))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(look.text(size))
                .line_height(look.text(size * 1.45))
                .text_color(color)
                .child(text.to_owned()),
        )
}

/// `was` struck through on red, an arrow, `now` on green.
pub fn diff(look: &Look, t: &Nt, was: Option<&str>, now: &str) -> gpui::Div {
    let side = |text: &str, bg: Hsla, fg: Hsla, struck: bool| {
        div()
            .px(look.px(6.))
            .py(look.px(2.))
            .rounded(look.px(5.))
            .bg(bg)
            .text_color(fg)
            .text_size(look.text(13.))
            .line_clamp(2)
            .when(struck, |d| d.line_through())
            .child(text.to_owned())
    };
    div()
        .flex()
        .flex_col()
        .gap(look.px(6.))
        .children(was.map(|w| {
            div()
                .flex()
                .items_center()
                .gap(look.px(6.))
                .child(side(w, t.bad_bg, t.muted, true))
                .child(icon(look, "right", 14., t.faint))
        }))
        .child(div().flex().child(side(now, t.ok_bg, t.text, false)))
}

/// A label above a group (sentence case, faint) with an optional count.
pub fn label(look: &Look, t: &Nt, text: &str, n: Option<(usize, Tone)>) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(look.px(8.))
        .text_size(look.text(13.))
        .text_color(t.muted)
        .child(text.to_owned())
        .children(n.map(|(n, tone)| count(look, t, n, tone)))
}

/// The dotted ground: one 2-point dot every 24 points, from `offset`
/// (map points scrolled by the pan) at `zoom`.
pub fn ground(look: &Look, t: &Nt, offset: (f32, f32), zoom: f32) -> AnyElement {
    let dot = t.dot;
    let step = 24. * zoom.clamp(0.5, 2.);
    let unit = f32::from(look.px(1.));
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
            let s = step * unit;
            let ox = (-offset.0 * zoom * unit).rem_euclid(s);
            let oy = (-offset.1 * zoom * unit).rem_euclid(s);
            let d = 2. * unit;
            let mut y = oy;
            while y < h {
                let mut x = ox;
                while x < w {
                    window.paint_quad(
                        gpui::fill(
                            gpui::Bounds::new(
                                point(b.origin.x + px(x), b.origin.y + px(y)),
                                gpui::size(px(d), px(d)),
                            ),
                            dot,
                        )
                        .corner_radii(px(d / 2.)),
                    );
                    x += s;
                }
                y += s;
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

/// A connector: a polyline of points (in pixels, from the canvas's
/// origin) with a 5-point ring at each end.
#[derive(Clone, Debug)]
pub struct Wire {
    pub points: Vec<(f32, f32)>,
    pub color: Hsla,
    pub width: f32,
    pub rings: bool,
}

/// Paints wires into a canvas laid over the area they join.
pub fn wires(wires: Vec<Wire>, ring_fill: Hsla) -> AnyElement {
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let o = b.origin;
            let at = |p: (f32, f32)| -> Point<Pixels> { point(o.x + px(p.0), o.y + px(p.1)) };
            for w in &wires {
                if w.points.len() < 2 {
                    continue;
                }
                let mut path = PathBuilder::stroke(px(w.width));
                path.move_to(at(w.points[0]));
                for p in &w.points[1..] {
                    path.line_to(at(*p));
                }
                if let Ok(p) = path.build() {
                    window.paint_path(p, w.color);
                }
                if w.rings {
                    for end in [w.points[0], *w.points.last().expect("two points")] {
                        let r = 3.;
                        let bounds = gpui::Bounds::new(
                            point(o.x + px(end.0 - r), o.y + px(end.1 - r)),
                            gpui::size(px(2. * r), px(2. * r)),
                        );
                        window.paint_quad(gpui::quad(
                            bounds,
                            px(r),
                            ring_fill,
                            px(1.5),
                            w.color,
                            gpui::BorderStyle::Solid,
                        ));
                    }
                }
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

/// The strip: the day as bars (amber where you corrected an agent), the
/// night as an indigo zone with the pass's bars, the morning to a now line.
pub fn strip(look: &Look, t: &Nt, s: &hippo_view::night::strip::Strip, h: f32) -> AnyElement {
    let beats = s.beats.clone();
    let corrections = s.corrections.clone();
    let night = (s.x(s.night_start), s.x(s.night_end));
    let pass = match (s.pass_start, s.pass_end) {
        (Some(a), Some(b)) => Some((s.x(a), s.x(b))),
        (Some(a), None) | (None, Some(a)) => Some((s.x(a), s.x(a))),
        _ => None,
    };
    let (bar, you, nightc, night_bg, now_c, surface, line) = (
        t.faint.opacity(0.5),
        t.you,
        t.night,
        t.night_bg,
        t.text,
        t.surface,
        t.line,
    );
    let unit = f32::from(look.px(1.));
    div()
        .h(look.px(h))
        .w_full()
        .rounded(look.px(10.))
        .border(look.fixed(1.))
        .border_color(line)
        .bg(surface)
        .overflow_hidden()
        .child(
            canvas(
                |_, _, _| {},
                move |b, _, window, _| {
                    let o = b.origin;
                    let (w, hh) = (f32::from(b.size.width), f32::from(b.size.height));
                    let pad = 6. * unit;
                    // The night zone.
                    let (n0, n1) = (night.0 * w, night.1 * w);
                    window.paint_quad(gpui::fill(
                        gpui::Bounds::new(
                            point(o.x + px(n0), o.y),
                            gpui::size(px((n1 - n0).max(1.)), px(hh)),
                        ),
                        night_bg,
                    ));
                    let n = beats.len().max(1);
                    let slot = (w - 2. * pad) / n as f32;
                    let peak = beats.iter().copied().max().unwrap_or(0).max(4) as f32;
                    for (i, v) in beats.iter().enumerate() {
                        if *v == 0 {
                            continue;
                        }
                        let frac = (*v as f32 / peak).sqrt().min(1.);
                        let bh = (frac * (hh - 2. * pad)).max(2. * unit);
                        let x = pad + i as f32 * slot;
                        let corr = corrections.contains(&i);
                        window.paint_quad(gpui::fill(
                            gpui::Bounds::new(
                                point(o.x + px(x), o.y + px(hh - pad - bh)),
                                gpui::size(px((slot - 1.5 * unit).max(unit)), px(bh)),
                            ),
                            if corr { you } else { bar },
                        ));
                    }
                    // The pass.
                    if let Some((a, z)) = pass {
                        let x0 = a * w;
                        let x1 = (z * w).max(x0 + 3. * unit);
                        for (k, frac) in [0.8f32, 0.5].iter().enumerate() {
                            let bh = frac * (hh - 2. * pad);
                            let x = x0 + k as f32 * 6. * unit;
                            if x > x1 + 6. * unit {
                                break;
                            }
                            window.paint_quad(gpui::fill(
                                gpui::Bounds::new(
                                    point(o.x + px(x), o.y + px(hh - pad - bh)),
                                    gpui::size(px(4. * unit), px(bh)),
                                ),
                                nightc,
                            ));
                        }
                    }
                    // Now.
                    window.paint_quad(gpui::fill(
                        gpui::Bounds::new(
                            point(o.x + px(w - 3. * unit), o.y + px(pad / 2.)),
                            gpui::size(px(2. * unit), px(hh - pad)),
                        ),
                        now_c,
                    ));
                },
            )
            .size_full(),
        )
        .into_any_element()
}

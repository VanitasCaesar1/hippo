//! Markdown drawn with GPUI: what `hippo_view::markdown` parses, as wrapped
//! text with bold, italic, monospace code and links. Shared by the session
//! tab and the note tab.

use std::ops::Range;
use std::rc::Rc;

use gpui::{
    App, FontStyle, FontWeight, HighlightStyle, InteractiveText, SharedString, StrikethroughStyle,
    StyledText, UnderlineStyle, Window, div, prelude::*,
};
use hippo_view::markdown::{self, Block, Span};
use hippo_view::text::visible;

use super::theme::Look;

/// Opens the note a `hippo-note:<n>` link names, by its index.
pub type NoteLinks = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// Draws spans as one wrapped text: bold, italic, code in monospace on the
/// elevated colour, links in the accent colour: a web link opens in the
/// browser, a note link through `notes`.
fn spans_element(
    key: SharedString,
    spans: &[Span],
    look: &Look,
    notes: Option<&NoteLinks>,
) -> gpui::AnyElement {
    let mut text = String::new();
    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    let mut mono: Vec<(Range<usize>, SharedString)> = Vec::new();
    let mut links: Vec<(Range<usize>, String)> = Vec::new();
    for span in spans {
        let start = text.len();
        text.push_str(&visible(&span.text));
        let range = start..text.len();
        if range.is_empty() {
            continue;
        }
        let mut style = HighlightStyle::default();
        let mut styled = false;
        if span.bold {
            style.font_weight = Some(FontWeight::BOLD);
            styled = true;
        }
        if span.italic {
            style.font_style = Some(FontStyle::Italic);
            styled = true;
        }
        if span.code {
            style.background_color = Some(look.c.elevated);
            mono.push((range.clone(), "Menlo".into()));
            styled = true;
        }
        if let Some(url) = &span.link {
            if url.starts_with("hippo-dangling:") {
                // A note link with no note: marked, not followed.
                style.color = Some(look.c.text_muted);
                style.font_style = Some(FontStyle::Italic);
                style.strikethrough = Some(StrikethroughStyle {
                    thickness: look.fixed(1.),
                    color: Some(look.c.text_muted),
                });
            } else {
                style.color = Some(look.c.text_accent);
                if !url.starts_with("hippo-note:") {
                    style.underline = Some(UnderlineStyle {
                        thickness: look.fixed(1.),
                        color: Some(look.c.text_accent),
                        wavy: false,
                    });
                }
                links.push((range.clone(), url.clone()));
            }
            styled = true;
        }
        if styled {
            highlights.push((range, style));
        }
    }
    let styled = StyledText::new(text)
        .with_highlights(highlights)
        .with_font_family_overrides(mono);
    if links.is_empty() {
        return styled.into_any_element();
    }
    let (ranges, urls): (Vec<_>, Vec<_>) = links.into_iter().unzip();
    let notes = notes.cloned();
    InteractiveText::new(key, styled)
        .on_click(ranges, move |i, window, cx| {
            let Some(url) = urls.get(i) else { return };
            match url.strip_prefix("hippo-note:") {
                Some(n) => {
                    if let (Some(open), Ok(n)) = (&notes, n.parse::<usize>()) {
                        open(n, window, cx);
                    }
                }
                None => cx.open_url(url),
            }
        })
        .into_any_element()
}

/// Monospace text on the elevated colour.
pub fn code_block(text: &str, look: &Look) -> gpui::AnyElement {
    div()
        .w_full()
        .px(look.px(8.))
        .py(look.px(6.))
        .rounded(look.px(4.))
        .bg(look.c.elevated)
        .font_family("Menlo")
        .text_size(look.text(12.))
        .line_height(gpui::relative(1.4))
        .child(visible(text))
        .into_any_element()
}

/// Markdown `text` as blocks.
pub fn markdown_element(
    key: &str,
    text: &str,
    look: &Look,
    notes: Option<&NoteLinks>,
) -> gpui::AnyElement {
    let blocks = markdown::parse(text);
    div()
        .flex()
        .flex_col()
        .gap(look.px(6.))
        .line_height(gpui::relative(1.45))
        .children(blocks.iter().enumerate().map(|(i, block)| {
            let key = SharedString::from(format!("{key}-{i}"));
            match block {
                Block::Heading { spans, .. } => div()
                    .pt(look.px(4.))
                    .font_weight(FontWeight::BOLD)
                    .child(spans_element(key, spans, look, notes))
                    .into_any_element(),
                Block::Paragraph(spans) => div()
                    .child(spans_element(key, spans, look, notes))
                    .into_any_element(),
                Block::Item {
                    marker,
                    depth,
                    spans,
                } => div()
                    .flex()
                    .pl(look.px(16. * *depth as f32))
                    .child(
                        div()
                            .w(look.px(20.))
                            .flex_shrink_0()
                            .text_color(look.c.text_muted)
                            .child(marker.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(spans_element(key, spans, look, notes)),
                    )
                    .into_any_element(),
                Block::Quote(spans) => div()
                    .border_l(look.fixed(2.))
                    .border_color(look.c.border)
                    .pl(look.px(10.))
                    .text_color(look.c.text_muted)
                    .child(spans_element(key, spans, look, notes))
                    .into_any_element(),
                Block::Code { text, .. } => code_block(text, look),
                Block::Rule => div()
                    .h(look.fixed(1.))
                    .my(look.px(4.))
                    .bg(look.c.border)
                    .into_any_element(),
            }
        }))
        .into_any_element()
}

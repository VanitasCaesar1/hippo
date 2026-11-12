//! Draws the card with the embedded DejaVu
//! subset, read by [`super::font`], and `tiny-skia` fills. No system font is
//! read, so the pixels are the same on every machine.

use super::font::{self, Font as Face};
use super::{png, Run, Words, NAME, TAGLINE};
use tiny_skia::{Color, FillRule, Paint, Path, PathBuilder, Pixmap, Rect, Shader, Transform};

pub const WIDTH: u32 = 1200;
pub const HEIGHT: u32 = 675;

const REGULAR: &[u8] = include_bytes!("../../assets/fonts/DejaVuSans.ttf");
const BOLD: &[u8] = include_bytes!("../../assets/fonts/DejaVuSans-Bold.ttf");
const MARK_SVG: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../brand/hippo-mark.svg"
));

const NAVY: (u8, u8, u8) = (0x1E, 0x2A, 0x4A);
const YELLOW: (u8, u8, u8) = (0xFF, 0xD6, 0x6B);
const MUTED: (u8, u8, u8) = (0xAE, 0xB8, 0xD6);
const SOFT: (u8, u8, u8) = (0xDC, 0xE2, 0xF2);
const WHITE: (u8, u8, u8) = (0xFF, 0xFF, 0xFF);

// The reference's geometry at 1200 px wide (1 cqw = 12 px).
const TOP: f32 = 74.4;
const SIDE: f32 = 78.0;
const BOTTOM: f32 = 60.0;
const DATE_SIZE: f32 = 22.8;
const BIG_SIZE: f32 = 63.6;
const BIG_LINE: f32 = 1.16;
const BIG_GAP: f32 = 24.0;
const LIST_SIZE: f32 = 25.8;
const LIST_LINE: f32 = 1.75;
const NAME_SIZE: f32 = 30.0;
const TAG_SIZE: f32 = 21.0;
const FOOT_LINE: f32 = 1.5;
const MARK_RIGHT: f32 = 72.0;
const MARK_TOP: f32 = 330.0;
const MARK_WIDTH: f32 = 228.0;
/// The space kept between the list and the footer.
const GUTTER: f32 = 24.0;

struct Font<'a> {
    face: Face<'a>,
}

impl<'a> Font<'a> {
    fn load(data: &'a [u8]) -> Font<'a> {
        Font {
            face: Face::parse(data).expect("the embedded font parses"),
        }
    }

    fn em(&self) -> f32 {
        f32::from(self.face.units_per_em)
    }

    fn ascent(&self, size: f32) -> f32 {
        f32::from(self.face.ascender) / self.em() * size
    }

    /// The height CSS gives `line-height: normal`.
    fn content(&self, size: f32) -> f32 {
        f32::from(self.face.ascender - self.face.descender + self.face.line_gap) / self.em() * size
    }

    fn glyph(&self, c: char) -> u16 {
        self.face
            .glyph(c)
            .or_else(|| self.face.glyph('?'))
            .unwrap_or(0)
    }

    /// Each glyph of `text` with its x offset, and the advance of the whole.
    fn layout(&self, text: &str, size: f32, spacing: f32) -> (Vec<(u16, f32)>, f32) {
        let scale = size / self.em();
        let mut x = 0.0;
        let mut out = Vec::new();
        let mut prev: Option<u16> = None;
        for c in text.chars() {
            let id = self.glyph(c);
            if let Some(p) = prev {
                x += f32::from(self.face.kerning(p, id)) * scale;
            }
            out.push((id, x));
            x += f32::from(self.face.advance(id)) * scale + spacing;
            prev = Some(id);
        }
        (out, x)
    }

    fn width(&self, text: &str, size: f32, spacing: f32) -> f32 {
        self.layout(text, size, spacing).1
    }

    /// Adds glyph `id`'s outline at `x`, `baseline`, scaled to `size`.
    fn outline(&self, pb: &mut PathBuilder, id: u16, x: f32, baseline: f32, size: f32) {
        let k = size / self.em();
        let pb = std::cell::RefCell::new(pb);
        for contour in self.face.contours(id) {
            font::walk(
                &contour,
                |px, py| pb.borrow_mut().move_to(x + px * k, baseline - py * k),
                |px, py| pb.borrow_mut().line_to(x + px * k, baseline - py * k),
                |cx, cy, px, py| {
                    pb.borrow_mut().quad_to(
                        x + cx * k,
                        baseline - cy * k,
                        x + px * k,
                        baseline - py * k,
                    )
                },
            );
            pb.borrow_mut().close();
        }
    }
}

fn paint(rgb: (u8, u8, u8), alpha: f32) -> Paint<'static> {
    let mut color = Color::from_rgba8(rgb.0, rgb.1, rgb.2, 255);
    color.set_alpha(alpha);
    Paint {
        shader: Shader::SolidColor(color),
        anti_alias: true,
        ..Paint::default()
    }
}

fn fill(pixmap: &mut Pixmap, path: Option<Path>, rgb: (u8, u8, u8), alpha: f32) {
    if let Some(path) = path {
        pixmap.fill_path(
            &path,
            &paint(rgb, alpha),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// Draws `text` with its left end at `x` and its baseline at `baseline`;
/// returns the advance.
#[allow(clippy::too_many_arguments)]
fn draw(
    pixmap: &mut Pixmap,
    font: &Font,
    text: &str,
    size: f32,
    spacing: f32,
    x: f32,
    baseline: f32,
    rgb: (u8, u8, u8),
) -> f32 {
    let (glyphs, width) = font.layout(text, size, spacing);
    let mut pb = PathBuilder::new();
    for (id, dx) in glyphs {
        font.outline(&mut pb, id, x + dx, baseline, size);
    }
    fill(pixmap, pb.finish(), rgb, 1.0);
    width
}

/// The baseline of a line box of height `line` whose top is `top` (CSS
/// centres the font's content area in the line box).
fn baseline(font: &Font, size: f32, top: f32, line: f32) -> f32 {
    top + (line - font.content(size)) / 2.0 + font.ascent(size)
}

/// The largest size up to `size` at which `width(size)` fits in `room`.
fn fit(size: f32, room: f32, width: impl Fn(f32) -> f32) -> f32 {
    let w = width(size);
    if w <= room {
        size
    } else {
        size * room / w
    }
}

/// Parses the mark's path (`M`, `m`, `L`, `l`, `H`, `h`, `V`, `v`, `C`,
/// `c`, `Z`, `z`) in the SVG's own units.
fn mark_path(d: &str, map: impl Fn(f32, f32) -> (f32, f32)) -> Option<Path> {
    let mut pb = PathBuilder::new();
    let mut tokens = Vec::new();
    let mut num = String::new();
    for c in d.chars() {
        if c.is_ascii_alphabetic() {
            if !num.is_empty() {
                tokens.push(std::mem::take(&mut num));
            }
            tokens.push(c.to_string());
        } else if c == ' ' || c == ',' || c == '\n' {
            if !num.is_empty() {
                tokens.push(std::mem::take(&mut num));
            }
        } else if c == '-' && !num.is_empty() {
            tokens.push(std::mem::take(&mut num));
            num.push(c);
        } else {
            num.push(c);
        }
    }
    if !num.is_empty() {
        tokens.push(num);
    }
    let mut i = 0;
    let (mut cx, mut cy, mut sx, mut sy) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    let mut cmd = 'M';
    let next = |i: &mut usize| -> Option<f32> {
        let v = tokens.get(*i)?.parse().ok()?;
        *i += 1;
        Some(v)
    };
    let to = |x: f32, y: f32| map(x, y);
    while i < tokens.len() {
        if let Some(c) = tokens[i].chars().next().filter(char::is_ascii_alphabetic) {
            cmd = c;
            i += 1;
            if c == 'z' || c == 'Z' {
                pb.close();
                cx = sx;
                cy = sy;
                continue;
            }
        }
        let rel = cmd.is_ascii_lowercase();
        let (ox, oy) = if rel { (cx, cy) } else { (0.0, 0.0) };
        match cmd.to_ascii_uppercase() {
            'M' => {
                let (x, y) = (next(&mut i)? + ox, next(&mut i)? + oy);
                let p = to(x, y);
                pb.move_to(p.0, p.1);
                (cx, cy, sx, sy) = (x, y, x, y);
                // Later pairs are line-tos.
                cmd = if rel { 'l' } else { 'L' };
            }
            'L' => {
                let (x, y) = (next(&mut i)? + ox, next(&mut i)? + oy);
                let p = to(x, y);
                pb.line_to(p.0, p.1);
                (cx, cy) = (x, y);
            }
            'H' => {
                let x = next(&mut i)? + ox;
                let p = to(x, cy);
                pb.line_to(p.0, p.1);
                cx = x;
            }
            'V' => {
                let y = next(&mut i)? + oy;
                let p = to(cx, y);
                pb.line_to(p.0, p.1);
                cy = y;
            }
            'C' => {
                let (x1, y1) = (next(&mut i)? + ox, next(&mut i)? + oy);
                let (x2, y2) = (next(&mut i)? + ox, next(&mut i)? + oy);
                let (x, y) = (next(&mut i)? + ox, next(&mut i)? + oy);
                let (a, b, c) = (to(x1, y1), to(x2, y2), to(x, y));
                pb.cubic_to(a.0, a.1, b.0, b.1, c.0, c.1);
                (cx, cy) = (x, y);
            }
            _ => return None,
        }
    }
    pb.finish()
}

/// The mark's path data from `brand/hippo-mark.svg`.
fn mark_d() -> &'static str {
    let start = MARK_SVG.find(" d=\"").expect("the mark has a path") + 4;
    let len = MARK_SVG[start..].find('"').expect("the path ends");
    &MARK_SVG[start..start + len]
}

/// Draws the card for `words` and returns the PNG.
pub fn render(words: &Words) -> Vec<u8> {
    let regular = Font::load(REGULAR);
    let bold = Font::load(BOLD);
    let mut pixmap = Pixmap::new(WIDTH, HEIGHT).expect("a 1200 x 675 pixmap");
    let (w, h) = (WIDTH as f32, HEIGHT as f32);
    pixmap.fill_rect(
        Rect::from_xywh(0.0, 0.0, w, h).expect("the card's rectangle"),
        &paint(NAVY, 1.0),
        Transform::identity(),
        None,
    );
    let room = w - 2.0 * SIDE;

    // The date, 0.02 em apart.
    let size = fit(DATE_SIZE, room, |s| regular.width(&words.date, s, 0.02 * s));
    let date_line = regular.content(DATE_SIZE);
    draw(
        &mut pixmap,
        &regular,
        &words.date,
        size,
        0.02 * size,
        SIDE,
        baseline(&regular, DATE_SIZE, TOP, date_line),
        MUTED,
    );

    // The sentence: three lines, bold, -0.02 em, the work time in yellow.
    let widest = |s: f32| {
        [&words.lead, &words.work, &words.tail]
            .iter()
            .map(|t| bold.width(t, s, -0.02 * s))
            .fold(0.0, f32::max)
    };
    let size = fit(BIG_SIZE, room, widest);
    let line = size * BIG_LINE;
    let mut top = TOP + date_line + BIG_GAP;
    for (text, rgb) in [
        (&words.lead, WHITE),
        (&words.work, YELLOW),
        (&words.tail, WHITE),
    ] {
        let base = baseline(&bold, size, top, line);
        draw(
            &mut pixmap,
            &bold,
            text,
            size,
            -0.02 * size,
            SIDE,
            base,
            rgb,
        );
        top += line;
    }

    // The footer, right-aligned at the bottom right.
    let right = w - SIDE;
    let foot_bottom = h - BOTTOM;
    let tag_top = foot_bottom - TAG_SIZE * FOOT_LINE;
    let name_top = tag_top - NAME_SIZE * FOOT_LINE;
    let tag_width = regular.width(TAGLINE, TAG_SIZE, 0.0);
    let name_width = bold.width(NAME, NAME_SIZE, 0.01 * NAME_SIZE);
    draw(
        &mut pixmap,
        &bold,
        NAME,
        NAME_SIZE,
        0.01 * NAME_SIZE,
        right - name_width,
        baseline(&bold, NAME_SIZE, name_top, NAME_SIZE * FOOT_LINE),
        WHITE,
    );
    draw(
        &mut pixmap,
        &regular,
        TAGLINE,
        TAG_SIZE,
        0.0,
        right - tag_width,
        baseline(&regular, TAG_SIZE, tag_top, TAG_SIZE * FOOT_LINE),
        MUTED,
    );

    // The four lines, bottom-aligned, numbers bold white, kept clear of
    // the footer (and within 62 % of the width, as the reference is).
    let list_room = (room * 0.62).min(right - tag_width.max(name_width) - GUTTER - SIDE);
    let run_width = |runs: &[Run], s: f32| {
        runs.iter()
            .map(|r| {
                let f = if r.bold { &bold } else { &regular };
                f.width(&r.text, s, 0.0)
            })
            .sum::<f32>()
    };
    let size = fit(LIST_SIZE, list_room, |s| {
        words
            .lines
            .iter()
            .map(|l| run_width(l, s))
            .fold(0.0, f32::max)
    });
    let line = LIST_SIZE * LIST_LINE;
    let mut top = foot_bottom - line * words.lines.len() as f32;
    for runs in &words.lines {
        let base = baseline(&regular, size, top, line);
        let mut x = SIDE;
        for run in runs {
            let (font, rgb) = if run.bold {
                (&bold, WHITE)
            } else {
                (&regular, SOFT)
            };
            x += draw(&mut pixmap, font, &run.text, size, 0.0, x, base, rgb);
        }
        top += line;
    }

    // The mark: viewBox 473 x 387 with the path in tenths, y flipped.
    let k = MARK_WIDTH / 473.0;
    let left = w - MARK_RIGHT - MARK_WIDTH;
    let path = mark_path(mark_d(), |x, y| {
        (left + 0.1 * x * k, MARK_TOP + (387.0 - 0.1 * y) * k)
    });
    fill(&mut pixmap, path, WHITE, 0.96);

    let mut rgb = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for px in pixmap.pixels() {
        let c = px.demultiply();
        rgb.extend_from_slice(&[c.red(), c.green(), c.blue()]);
    }
    png::encode(WIDTH, HEIGHT, &rgb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{words, Facts, Span};
    use time::macros::date;
    use time::Duration;

    #[test]
    fn the_mark_is_the_brand_drawing_in_its_frame() {
        assert!(MARK_SVG.contains("viewBox=\"0 0 473 387\""));
        assert!(MARK_SVG.contains("translate(0,387) scale(0.1,-0.1)"));
        let path = mark_path(mark_d(), |x, y| (0.1 * x, 387.0 - 0.1 * y)).unwrap();
        let b = path.bounds();
        assert!(b.left() >= 0.0 && b.right() <= 473.0, "{b:?}");
        assert!(b.top() >= 0.0 && b.bottom() <= 387.0, "{b:?}");
    }

    #[test]
    fn the_fonts_cover_every_character_the_card_can_draw() {
        for data in [REGULAR, BOLD] {
            let face = Face::parse(data).unwrap();
            for c in (' '..='~').chain(['·', '×', '–', '—', '…']) {
                let id = face.glyph(c).unwrap_or_else(|| panic!("{c:?}"));
                assert!(face.advance(id) > 0, "{c:?}");
                if !c.is_whitespace() {
                    assert!(!face.contours(id).is_empty(), "{c:?} has no outline");
                }
            }
        }
    }

    #[test]
    fn the_card_is_a_1200_by_675_png_and_the_same_every_time() {
        let f = Facts {
            span: Span::Today(date!(2026 - 11 - 10)),
            work: Duration::minutes(3394),
            threads: 13,
            subs: 538,
            merged: 66,
            waited: Duration::minutes(89),
            questions: 7,
            corrections: 0,
        };
        let a = render(&words(&f));
        let b = render(&words(&f));
        assert_eq!(a, b);
        assert_eq!(&a[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(u32::from_be_bytes(a[16..20].try_into().unwrap()), 1200);
        assert_eq!(u32::from_be_bytes(a[20..24].try_into().unwrap()), 675);
    }
}

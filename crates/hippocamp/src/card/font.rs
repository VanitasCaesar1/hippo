//! A minimal reader for the card's embedded TrueType fonts: the character map (format 4), horizontal metrics,
//! glyph outlines (simple and composite) and a legacy `kern` table
//! (format 0). It reads only fonts compiled into the binary, so a table it
//! cannot read is a bug caught by the tests, not a user's error.

use std::collections::HashMap;

/// A point of an outline, in font units, and whether it is on the curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
    pub on: bool,
}

/// One TrueType font.
pub struct Font<'a> {
    glyf: &'a [u8],
    loca: Vec<u32>,
    hmtx: &'a [u8],
    hmetrics: u16,
    cmap: HashMap<char, u16>,
    kern: HashMap<(u16, u16), i16>,
    pub units_per_em: u16,
    pub ascender: i16,
    pub descender: i16,
    pub line_gap: i16,
}

fn u16_at(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*d.get(at)?, *d.get(at + 1)?]))
}

fn i16_at(d: &[u8], at: usize) -> Option<i16> {
    u16_at(d, at).map(|v| v as i16)
}

fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *d.get(at)?,
        *d.get(at + 1)?,
        *d.get(at + 2)?,
        *d.get(at + 3)?,
    ]))
}

fn tables(data: &[u8]) -> Option<HashMap<[u8; 4], &[u8]>> {
    let count = usize::from(u16_at(data, 4)?);
    let mut out = HashMap::new();
    for i in 0..count {
        let rec = 12 + 16 * i;
        let tag: [u8; 4] = data.get(rec..rec + 4)?.try_into().ok()?;
        let offset = u32_at(data, rec + 8)? as usize;
        let len = u32_at(data, rec + 12)? as usize;
        out.insert(tag, data.get(offset..offset + len)?);
    }
    Some(out)
}

/// The Unicode BMP subtable (platform 3 encoding 1, or platform 0), format 4.
fn cmap(table: &[u8]) -> Option<HashMap<char, u16>> {
    let count = usize::from(u16_at(table, 2)?);
    let sub = (0..count).find_map(|i| {
        let rec = 4 + 8 * i;
        let (platform, encoding) = (u16_at(table, rec)?, u16_at(table, rec + 2)?);
        let offset = u32_at(table, rec + 4)? as usize;
        let unicode = platform == 0 || (platform == 3 && encoding == 1);
        (unicode && u16_at(table, offset)? == 4).then_some(offset)
    })?;
    let t = table.get(sub..)?;
    let segs = usize::from(u16_at(t, 6)? / 2);
    let ends = 14;
    let starts = ends + 2 * segs + 2;
    let deltas = starts + 2 * segs;
    let ranges = deltas + 2 * segs;
    let mut out = HashMap::new();
    for s in 0..segs {
        let (start, end) = (u16_at(t, starts + 2 * s)?, u16_at(t, ends + 2 * s)?);
        let delta = u16_at(t, deltas + 2 * s)?;
        let range = usize::from(u16_at(t, ranges + 2 * s)?);
        for code in start..=end {
            if code == 0xFFFF {
                break;
            }
            let gid = if range == 0 {
                code.wrapping_add(delta)
            } else {
                let at = ranges + 2 * s + range + 2 * usize::from(code - start);
                match u16_at(t, at)? {
                    0 => 0,
                    g => g.wrapping_add(delta),
                }
            };
            if gid != 0 {
                if let Some(c) = char::from_u32(u32::from(code)) {
                    out.insert(c, gid);
                }
            }
        }
    }
    Some(out)
}

/// Horizontal format 0 subtables of a version 0 `kern` table.
fn kern(table: Option<&&[u8]>) -> HashMap<(u16, u16), i16> {
    let mut out = HashMap::new();
    let Some(t) = table else { return out };
    let mut read = || -> Option<()> {
        let count = usize::from(u16_at(t, 2)?);
        let mut at = 4;
        for _ in 0..count {
            let len = usize::from(u16_at(t, at + 2)?);
            let coverage = u16_at(t, at + 4)?;
            if coverage >> 8 == 0 && coverage & 1 == 1 {
                let pairs = usize::from(u16_at(t, at + 6)?);
                for p in 0..pairs {
                    let rec = at + 14 + 6 * p;
                    out.insert((u16_at(t, rec)?, u16_at(t, rec + 2)?), i16_at(t, rec + 4)?);
                }
            }
            at += len;
        }
        Some(())
    };
    let _ = read();
    out
}

impl<'a> Font<'a> {
    /// Reads `data`; `None` when a table the card needs is missing.
    pub fn parse(data: &'a [u8]) -> Option<Font<'a>> {
        let t = tables(data)?;
        let head = t.get(b"head")?;
        let hhea = t.get(b"hhea")?;
        let maxp = t.get(b"maxp")?;
        let glyphs = usize::from(u16_at(maxp, 4)?);
        let long = i16_at(head, 50)? == 1;
        let loca_t = t.get(b"loca")?;
        let loca = (0..=glyphs)
            .map(|i| {
                if long {
                    u32_at(loca_t, 4 * i)
                } else {
                    u16_at(loca_t, 2 * i).map(|v| u32::from(v) * 2)
                }
            })
            .collect::<Option<Vec<u32>>>()?;
        Some(Font {
            glyf: t.get(b"glyf")?,
            loca,
            hmtx: t.get(b"hmtx")?,
            hmetrics: u16_at(hhea, 34)?,
            cmap: cmap(t.get(b"cmap")?)?,
            kern: kern(t.get(b"kern")),
            units_per_em: u16_at(head, 18)?,
            ascender: i16_at(hhea, 4)?,
            descender: i16_at(hhea, 6)?,
            line_gap: i16_at(hhea, 8)?,
        })
    }

    pub fn glyph(&self, c: char) -> Option<u16> {
        self.cmap.get(&c).copied()
    }

    pub fn advance(&self, gid: u16) -> u16 {
        let i = gid.min(self.hmetrics.saturating_sub(1));
        u16_at(self.hmtx, 4 * usize::from(i)).unwrap_or(0)
    }

    pub fn kerning(&self, left: u16, right: u16) -> i16 {
        self.kern.get(&(left, right)).copied().unwrap_or(0)
    }

    /// The contours of glyph `gid`, composites resolved.
    pub fn contours(&self, gid: u16) -> Vec<Vec<Point>> {
        let mut out = Vec::new();
        self.add(gid, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], 0, &mut out);
        out
    }

    fn data(&self, gid: u16) -> Option<&'a [u8]> {
        let i = usize::from(gid);
        let (start, end) = (*self.loca.get(i)? as usize, *self.loca.get(i + 1)? as usize);
        if start >= end {
            return None;
        }
        self.glyf.get(start..end)
    }

    /// Adds glyph `gid`'s contours through the transform `m` (a, b, c, d,
    /// e, f: x' = a·x + c·y + e, y' = b·x + d·y + f).
    fn add(&self, gid: u16, m: [f32; 6], depth: u8, out: &mut Vec<Vec<Point>>) {
        let Some(g) = self.data(gid) else { return };
        let Some(n) = i16_at(g, 0) else { return };
        let map = |x: f32, y: f32| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]);
        if n >= 0 {
            if let Some(contours) = simple(g, n as usize) {
                for c in contours {
                    out.push(
                        c.into_iter()
                            .map(|p| {
                                let (x, y) = map(p.x, p.y);
                                Point { x, y, on: p.on }
                            })
                            .collect(),
                    );
                }
            }
            return;
        }
        if depth > 4 {
            return;
        }
        let mut at = 10;
        loop {
            let (Some(flags), Some(child)) = (u16_at(g, at), u16_at(g, at + 2)) else {
                return;
            };
            at += 4;
            let (dx, dy) = if flags & 0x1 != 0 {
                let v = (i16_at(g, at), i16_at(g, at + 2));
                at += 4;
                (f32::from(v.0.unwrap_or(0)), f32::from(v.1.unwrap_or(0)))
            } else {
                let v = (g.get(at).copied(), g.get(at + 1).copied());
                at += 2;
                (
                    f32::from(v.0.unwrap_or(0) as i8),
                    f32::from(v.1.unwrap_or(0) as i8),
                )
            };
            let f2 = |at: usize| f32::from(i16_at(g, at).unwrap_or(0)) / 16384.0;
            let (mut a, mut b, mut c, mut d) = (1.0, 0.0, 0.0, 1.0);
            if flags & 0x8 != 0 {
                a = f2(at);
                d = a;
                at += 2;
            } else if flags & 0x40 != 0 {
                a = f2(at);
                d = f2(at + 2);
                at += 4;
            } else if flags & 0x80 != 0 {
                a = f2(at);
                b = f2(at + 2);
                c = f2(at + 4);
                d = f2(at + 6);
                at += 8;
            }
            // Point-matching offsets (flag 0x2 clear) do not occur in DejaVu.
            let (ex, ey) = map(dx, dy);
            let child_m = [
                m[0] * a + m[2] * b,
                m[1] * a + m[3] * b,
                m[0] * c + m[2] * d,
                m[1] * c + m[3] * d,
                ex,
                ey,
            ];
            self.add(child, child_m, depth + 1, out);
            if flags & 0x20 == 0 {
                return;
            }
        }
    }
}

/// The contours of a simple glyph with `n` contours.
fn simple(g: &[u8], n: usize) -> Option<Vec<Vec<Point>>> {
    let ends = (0..n)
        .map(|i| u16_at(g, 10 + 2 * i).map(usize::from))
        .collect::<Option<Vec<_>>>()?;
    let points = ends.last().map_or(0, |e| e + 1);
    let ins = usize::from(u16_at(g, 10 + 2 * n)?);
    let mut at = 12 + 2 * n + ins;
    let mut flags = Vec::with_capacity(points);
    while flags.len() < points {
        let f = *g.get(at)?;
        at += 1;
        flags.push(f);
        if f & 0x8 != 0 {
            let repeat = *g.get(at)?;
            at += 1;
            for _ in 0..repeat {
                flags.push(f);
            }
        }
    }
    flags.truncate(points);
    let mut coords = |short: u8, same: u8| -> Option<Vec<f32>> {
        let mut v = 0i32;
        let mut out = Vec::with_capacity(points);
        for &f in &flags {
            if f & short != 0 {
                let d = i32::from(*g.get(at)?);
                at += 1;
                v += if f & same != 0 { d } else { -d };
            } else if f & same == 0 {
                v += i32::from(i16_at(g, at)?);
                at += 2;
            }
            out.push(v as f32);
        }
        Some(out)
    };
    let xs = coords(0x2, 0x10)?;
    let ys = coords(0x4, 0x20)?;
    let mut out = Vec::with_capacity(n);
    let mut start = 0;
    for end in ends {
        out.push(
            (start..=end)
                .map(|i| Point {
                    x: xs[i],
                    y: ys[i],
                    on: flags[i] & 1 != 0,
                })
                .collect(),
        );
        start = end + 1;
    }
    Some(out)
}

/// Walks a TrueType contour as move, line, quadratic and close steps,
/// adding the implied on-curve point between two off-curve points.
pub fn walk(
    contour: &[Point],
    mut move_to: impl FnMut(f32, f32),
    mut line_to: impl FnMut(f32, f32),
    mut quad_to: impl FnMut(f32, f32, f32, f32),
) {
    let n = contour.len();
    if n == 0 {
        return;
    }
    let mid = |a: Point, b: Point| Point {
        x: (a.x + b.x) / 2.0,
        y: (a.y + b.y) / 2.0,
        on: true,
    };
    // Start on an on-curve point: the first, else the last, else the
    // midpoint of the two.
    let (start, first) = if contour[0].on {
        (contour[0], 1)
    } else if contour[n - 1].on {
        (contour[n - 1], 0)
    } else {
        (mid(contour[n - 1], contour[0]), 0)
    };
    move_to(start.x, start.y);
    let mut control: Option<Point> = None;
    let rest = (first..n).map(|i| contour[i]).chain(std::iter::once(start));
    for p in rest {
        match (control, p.on) {
            (None, true) => line_to(p.x, p.y),
            (None, false) => control = Some(p),
            (Some(c), true) => {
                quad_to(c.x, c.y, p.x, p.y);
                control = None;
            }
            (Some(c), false) => {
                let m = mid(c, p);
                quad_to(c.x, c.y, m.x, m.y);
                control = Some(p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_square_walks_as_four_lines() {
        let p = |x, y| Point { x, y, on: true };
        let steps = std::cell::RefCell::new(Vec::new());
        walk(
            &[p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0), p(0.0, 1.0)],
            |x, y| steps.borrow_mut().push(format!("M{x},{y}")),
            |x, y| steps.borrow_mut().push(format!("L{x},{y}")),
            |_, _, _, _| steps.borrow_mut().push("Q".into()),
        );
        assert_eq!(steps.into_inner(), ["M0,0", "L1,0", "L1,1", "L0,1", "L0,0"]);
    }

    #[test]
    fn two_off_curve_points_imply_one_on_the_curve() {
        let p = |x, y, on| Point { x, y, on };
        let mut steps = Vec::new();
        let quads = std::cell::RefCell::new(Vec::new());
        walk(
            &[
                p(0.0, 0.0, true),
                p(2.0, 2.0, false),
                p(4.0, 2.0, false),
                p(6.0, 0.0, true),
            ],
            |x, y| steps.push((x, y)),
            |_, _| {},
            |cx, cy, x, y| quads.borrow_mut().push((cx, cy, x, y)),
        );
        assert_eq!(
            quads.into_inner(),
            [(2.0, 2.0, 3.0, 2.0), (4.0, 2.0, 6.0, 0.0)]
        );
    }
}

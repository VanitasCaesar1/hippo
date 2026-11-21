//! Text that fits its box: a width estimate for the app's sans-serif and
//! the size and line breaks that fit a string into a width.
//!
//! The estimate is on the wide side of the system UI font (SF Pro, Inter
//! and Helvetica all come in under it for ordinary words), so a label the
//! layout says fits does fit when it is shaped. The app still checks the
//! shaped width and draws a size smaller rather than cutting a label, if a
//! font ever runs wider.

/// The width of `s` in ems: per-character advances on the wide side of a
/// UI sans-serif; bold adds 7 %.
pub fn ems(s: &str, bold: bool) -> f32 {
    let w: f32 = s.chars().map(advance).sum();
    if bold {
        w * 1.07
    } else {
        w
    }
}

/// The width of `s` at `size`, in the same units as `size`.
pub fn width(s: &str, size: f32, bold: bool) -> f32 {
    ems(s, bold) * size
}

fn advance(c: char) -> f32 {
    match c {
        ' ' => 0.28,
        'i' | 'l' | 'j' | '!' | '|' | '.' | ',' | ':' | ';' | '\'' | '·' => 0.3,
        'f' | 't' | 'r' | '(' | ')' | '[' | ']' | '/' | '-' => 0.42,
        'm' | 'w' => 0.9,
        'M' | 'W' => 1.0,
        '—' | '…' | '×' => 1.0,
        '0'..='9' => 0.64,
        // Capitals, a little wider than the system font's.
        'I' => 0.32,
        'J' | 'L' => 0.54,
        'E' | 'F' => 0.56,
        'S' | 'T' | 'Z' | 'P' => 0.6,
        'B' | 'R' | 'K' | 'Y' | 'X' | 'V' => 0.64,
        'A' | 'C' | 'D' | 'U' | 'H' | 'N' => 0.69,
        'G' | 'O' | 'Q' => 0.75,
        'a'..='z' => 0.56,
        c if c.is_ascii() => 0.64,
        // Wide scripts and symbols: a full em.
        _ => 1.0,
    }
}

/// A string set at one size on one or more lines.
#[derive(Debug, Clone, PartialEq)]
pub struct Fitted {
    pub lines: Vec<String>,
    pub size: f32,
}

/// One line when that costs at most this share of the largest size.
pub const ONE_LINE: f32 = 0.7;

/// The largest size from `max` down to `min` (in steps of 1) at which `s`
/// breaks at spaces into at most `lines` lines no wider than `w`, except
/// that one line down to [`ONE_LINE`] × `max` comes first. When even `min`
/// does not fit, the words that fit at `min` and an ellipsis.
pub fn fit(s: &str, w: f32, max: f32, min: f32, lines: usize, bold: bool) -> Fitted {
    let s = s.trim();
    let lines = lines.max(1);
    if lines > 1 {
        let one = fit(s, w, max, (max * ONE_LINE).max(min), 1, bold);
        if !one.lines[0].ends_with('…') {
            return one;
        }
    }
    let mut size = max;
    loop {
        if let Some(l) = wrap(s, w, size, lines, bold) {
            return Fitted { lines: l, size };
        }
        if size - 1. < min {
            break;
        }
        size -= 1.;
    }
    let size = min;
    Fitted {
        lines: shorten(s, w, size, lines, bold),
        size,
    }
}

/// Breaks `s` at spaces into at most `n` lines no wider than `w` at
/// `size`; `None` when it does not fit.
pub fn wrap(s: &str, w: f32, size: f32, n: usize, bold: bool) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        let next = if cur.is_empty() {
            word.to_owned()
        } else {
            format!("{cur} {word}")
        };
        if width(&next, size, bold) <= w {
            cur = next;
            continue;
        }
        if cur.is_empty() || width(word, size, bold) > w {
            return None;
        }
        out.push(std::mem::replace(&mut cur, word.to_owned()));
        if out.len() >= n {
            return None;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    (out.len() <= n).then_some(out)
}

/// `s` in at most `n` lines of `w` at `size`, the last cut at a character
/// and ended with `…`.
fn shorten(s: &str, w: f32, size: f32, n: usize, bold: bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest: &str = s;
    while out.len() + 1 < n {
        // The longest run of whole words that fits.
        let mut end = 0;
        for (i, _) in rest.match_indices(' ').chain([(rest.len(), "")]) {
            if width(&rest[..i], size, bold) <= w {
                end = i;
            } else {
                break;
            }
        }
        if end == 0 {
            break;
        }
        out.push(rest[..end].to_owned());
        rest = rest[end..].trim_start();
    }
    let mut cut = String::new();
    for c in rest.chars() {
        let mut next = cut.clone();
        next.push(c);
        if width(&format!("{}…", next.trim_end()), size, bold) > w {
            break;
        }
        cut = next;
    }
    out.push(format!("{}…", cut.trim_end()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_title_takes_the_largest_size_on_one_line() {
        let f = fit("Runbooks", 300., 56., 20., 2, true);
        assert_eq!(f.size, 56.);
        assert_eq!(f.lines, ["Runbooks"]);
    }

    #[test]
    fn one_line_comes_first_when_it_costs_little() {
        let f = fit("START HERE", 312., 48., 22., 3, true);
        assert_eq!(f.lines, ["START HERE"]);
        assert!(f.size >= 48. * ONE_LINE);
    }

    #[test]
    fn a_long_title_wraps_before_it_shrinks_below_the_floor() {
        let f = fit("Architecture decision records", 300., 56., 20., 2, true);
        assert_eq!(f.lines.len(), 2, "{f:?}");
        for l in &f.lines {
            assert!(width(l, f.size, true) <= 300.);
        }
    }

    #[test]
    fn what_cannot_fit_is_cut_with_an_ellipsis_and_still_fits() {
        let s = "Averyveryverylongsinglewordthatnevershouldfitinsideanarrowbox";
        let f = fit(s, 120., 30., 12., 1, false);
        assert_eq!(f.size, 12.);
        assert!(f.lines[0].ends_with('…'));
        assert!(width(&f.lines[0], f.size, false) <= 120.);
    }
}

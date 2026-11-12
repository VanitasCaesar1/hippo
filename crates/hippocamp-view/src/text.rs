//! Escaping for text from hippocamp's files.
//!
//! A note title, a session task or a theme name can hold characters a person
//! cannot see but that change what they read: bidirectional controls, zero-width
//! characters, tags. Everything from a file that the window shows goes through
//! [`visible`].

use hippocamp_core::packs::files::is_denied;

/// `text` with every denied code point and every carriage return written as
/// `\u{XXXX}` (four or more upper-case hex digits). Tab and line feed stay.
///
/// The denied set is [`hippocamp_core::packs::files::is_denied`], the one table
/// that the pack check uses too.
pub fn visible(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '\r' || is_denied(c) {
            out.push_str(&format!("\\u{{{:04X}}}", u32::from(c)));
        } else {
            out.push(c);
        }
    }
    out
}

/// `text` with every 12-hour time (`6:10pm`, `5am`, `11:30 PM`) written on
/// the 24-hour clock (`18:10`, `05:00`, `23:30`), as the app writes times
/// everywhere. Anything else is kept.
pub fn twenty_four(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < b.len() {
        let starts = b[i].is_ascii_digit() && (i == 0 || !b[i - 1].is_ascii_alphanumeric());
        if starts {
            if let Some((len, hm)) = twelve_hour_at(&text[i..]) {
                out.push_str(&hm);
                i += len;
                continue;
            }
        }
        let c = text[i..].chars().next().expect("in bounds");
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// A 12-hour time at the start of `s`: its length and its 24-hour form.
fn twelve_hour_at(s: &str) -> Option<(usize, String)> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && i < 2 && b[i].is_ascii_digit() {
        i += 1;
    }
    let hour: u32 = s[..i].parse().ok()?;
    let mut minute = 0;
    if b.get(i) == Some(&b':')
        && b.get(i + 1..i + 3)
            .is_some_and(|m| m.iter().all(u8::is_ascii_digit))
    {
        minute = s[i + 1..i + 3].parse().ok()?;
        i += 3;
    }
    let mut j = i;
    if b.get(j) == Some(&b' ') {
        j += 1;
    }
    let mer = s.get(j..j + 2)?.to_ascii_lowercase();
    let pm = match mer.as_str() {
        "am" => false,
        "pm" => true,
        _ => return None,
    };
    if b.get(j + 2).is_some_and(|c| c.is_ascii_alphanumeric())
        || !(1..=12).contains(&hour)
        || minute > 59
    {
        return None;
    }
    let h = match (hour, pm) {
        (12, false) => 0,
        (12, true) => 12,
        (h, false) => h,
        (h, true) => h + 12,
    };
    Some((j + 2, format!("{h:02}:{minute:02}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twelve_hour_times_read_on_the_24_hour_clock() {
        assert_eq!(twenty_four("resets 6:10pm"), "resets 18:10");
        assert_eq!(
            twenty_four("resets 5am (Australia/Darwin)"),
            "resets 05:00 (Australia/Darwin)"
        );
        assert_eq!(twenty_four("12:30 AM and 12pm"), "00:30 and 12:00");
        assert_eq!(twenty_four("resets Thu 01:00"), "resets Thu 01:00");
        assert_eq!(twenty_four("13pm, 4amp, x5pm"), "13pm, 4amp, x5pm");
    }
}

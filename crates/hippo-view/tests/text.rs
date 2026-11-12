use hippo_view::text::visible;

fn escaped(c: char) -> String {
    format!("\\u{{{:04X}}}", u32::from(c))
}

#[test]
fn plain_text_and_tab_and_newline_pass_through() {
    let text = "Webhook retries\ttab\nnext line, é, 日本語, 🙂";
    assert_eq!(visible(text), text);
}

#[test]
fn a_carriage_return_is_escaped() {
    assert_eq!(visible("a\r\nb"), "a\\u{000D}\nb");
}

#[test]
fn every_denied_range_is_escaped_at_both_ends() {
    // The denied ranges: the first and last code point of each range.
    let ranges: &[(u32, u32)] = &[
        (0x0000, 0x0008),
        (0x000B, 0x000C),
        (0x000E, 0x001F),
        (0x007F, 0x009F),
        (0x00AD, 0x00AD),
        (0x034F, 0x034F),
        (0x061C, 0x061C),
        (0x115F, 0x1160),
        (0x17B4, 0x17B5),
        (0x180B, 0x180F),
        (0x200B, 0x200F),
        (0x2028, 0x202E),
        (0x2060, 0x206F),
        (0x3164, 0x3164),
        (0xE000, 0xF8FF),
        (0xFE00, 0xFE0F),
        (0xFEFF, 0xFEFF),
        (0xFFA0, 0xFFA0),
        (0xFFF0, 0xFFFB),
        (0x1BCA0, 0x1BCA3),
        (0x1D173, 0x1D17A),
        (0xE0000, 0xE0FFF),
        (0xF0000, 0xFFFFD),
        (0x10_0000, 0x10_FFFD),
    ];
    for &(lo, hi) in ranges {
        for cp in [lo, hi] {
            let c = char::from_u32(cp).unwrap();
            assert_eq!(
                visible(&format!("a{c}b")),
                format!("a{}b", escaped(c)),
                "{cp:X}"
            );
        }
    }
}

#[test]
fn named_cases() {
    // Bidirectional override, zero-width space, a tag character, a variation
    // selector and a private-use character.
    assert_eq!(visible("\u{202E}x"), "\\u{202E}x");
    assert_eq!(visible("a\u{200B}b"), "a\\u{200B}b");
    assert_eq!(visible("\u{E0041}"), "\\u{E0041}");
    assert_eq!(visible("\u{FE0F}"), "\\u{FE0F}");
    assert_eq!(visible("\u{E000}"), "\\u{E000}");
    assert_eq!(visible("\u{1}"), "\\u{0001}");
}

#[test]
fn the_code_points_next_to_a_range_stay() {
    for c in [
        '\u{09}', '\u{0A}', '\u{20}', '\u{7E}', '\u{A0}', '\u{AE}', '\u{2027}', '\u{202F}',
    ] {
        assert_eq!(visible(&c.to_string()), c.to_string(), "{:X}", u32::from(c));
    }
}

//! Text input rules shared by every form that publishes or stores text.
//!
//! Text is normalised: CRLF becomes LF and
//! leading and trailing whitespace is trimmed; a required field must be
//! non-empty after that. Titles and other one-line texts may not contain
//! control characters (U+0000 to U+001F, U+007F); bodies may contain line
//! feeds and tabs and no other control characters. One-line texts are
//! measured in Unicode scalar values, bodies in UTF-8 bytes.

use crate::error::FormError;

/// CRLF becomes LF; leading and trailing whitespace is trimmed.
pub fn normalise(raw: &str) -> String {
    raw.replace("\r\n", "\n").trim().to_string()
}

fn is_control(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}')
}

/// A one-line text (title, note, description, model name): normalised,
/// non-empty, with no control character, at most `max` scalar values.
pub fn one_line(field: &str, raw: &str, max: usize) -> Result<String, FormError> {
    let text = normalise(raw);
    if text.is_empty() {
        return Err(FormError::new(field, "is required"));
    }
    if text.chars().any(is_control) {
        return Err(FormError::new(field, "may not contain control characters"));
    }
    if text.chars().count() > max {
        return Err(FormError::new(
            field,
            format!("must be at most {max} characters"),
        ));
    }
    Ok(text)
}

/// A body: normalised, with no control character but line feed and tab, at
/// most `max_bytes` UTF-8 bytes. It may be empty; a caller that requires it
/// checks that.
pub fn body(field: &str, raw: &str, max_bytes: usize) -> Result<String, FormError> {
    let text = normalise(raw);
    if text
        .chars()
        .any(|c| is_control(c) && c != '\n' && c != '\t')
    {
        return Err(FormError::new(field, "may not contain control characters"));
    }
    if text.len() > max_bytes {
        return Err(FormError::new(
            field,
            format!("must be at most {max_bytes} bytes"),
        ));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalise_converts_crlf_and_trims() {
        assert_eq!(normalise("  a\r\nb \r\n"), "a\nb");
    }

    #[test]
    fn one_line_rules() {
        assert_eq!(one_line("title", "  hi  ", 5).as_deref(), Ok("hi"));
        assert_eq!(
            one_line("title", " \n ", 5),
            Err(FormError::new("title", "is required"))
        );
        assert!(one_line("title", "a\tb", 5).is_err());
        assert!(one_line("title", "a\nb", 5).is_err());
        assert!(one_line("title", "a\u{7f}", 5).is_err());
        // Scalars, not bytes: five two-byte characters fit in 5.
        assert!(one_line("title", "ééééé", 5).is_ok());
        assert!(one_line("title", "éééééé", 5).is_err());
    }

    #[test]
    fn body_rules() {
        assert_eq!(body("body", "a\r\n\tb\r\n", 10).as_deref(), Ok("a\n\tb"));
        assert!(body("body", "a\u{0}b", 10).is_err());
        assert!(body("body", "a\rb", 10).is_err());
        // Bytes, not scalars: three two-byte characters are 6 bytes.
        assert!(body("body", "ééé", 6).is_ok());
        assert!(body("body", "ééé", 5).is_err());
        assert_eq!(body("body", "  ", 5).as_deref(), Ok(""));
    }
}

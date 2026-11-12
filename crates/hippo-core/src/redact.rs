//! Redaction, the only way text enters the index (Invariant 2).
//!
//! Best-effort pattern matching over ten kinds, applied in this order:
//! `pem`, `aws`, `github`, `provider`, `jwt`, `auth`, `kv`, `email`,
//! `aadhaar`, `phone`. The entire match, including any key or header name,
//! becomes `[redacted:<kind>]`. `kv` redacts secret-shaped values only:
//! the value must hold both a letter and a digit. The key and the value may
//! each be quoted, as a JSON key and value are (`"token": "value"`), and the
//! quotes may be backslash-escaped, as inside a JSON string
//! (`KEY=\"value\"`). `aadhaar` and `phone` over-match numeric ids; that cost
//! is accepted, except inside a UUID, which they never touch, and `email`
//! needs a domain ending in a label of letters, so a package specifier
//! (`@scope/pkg@1.2.3`, `react@18.3.1`) stays as written.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::{Captures, NoExpand, Regex};

/// Text that has been through [`redact`]. Its field is private, so only
/// `redact` makes one, and the index can require it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redacted(String);

impl Redacted {
    /// The redacted text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Every expression with its kind, in the order they apply.
const PATTERNS: [(&str, &str); 15] = [
    (
        "pem",
        r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
    ),
    ("aws", r"AKIA[0-9A-Z]{16}"),
    ("github", r"gh[pousr]_[A-Za-z0-9]{36,}"),
    ("github", r"github_pat_[A-Za-z0-9_]{22,}"),
    ("provider", r"sk-ant-[A-Za-z0-9_-]{20,}"),
    ("provider", r"sk-[A-Za-z0-9]{20,}"),
    ("provider", r"sk_(live|test)_[A-Za-z0-9]{16,}"),
    ("provider", r"xox[abp]-[A-Za-z0-9-]{10,}"),
    ("provider", r"AIza[A-Za-z0-9_-]{35}"),
    (
        "jwt",
        r"eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
    ),
    ("auth", r"(?i)authorization:\s*\S+\s+\S+"),
    (
        "kv",
        r#"(?i)\\?["']?\b[\w.-]*(secret|token|password|passwd|api_key|apikey)[\w.-]*\\?["']?\s*[=:]\s*\\?["']?([A-Za-z0-9+/=_-]{16,})\\?["']?"#,
    ),
    ("email", r"[\w.+-]+@[\w-]+(?:\.[\w-]+)*\.[A-Za-z]{2,}\b"),
    ("aadhaar", r"\b[2-9]\d{3}[\s-]?\d{4}[\s-]?\d{4}\b"),
    ("phone", r"(?:\+91[\s-]?)?\b[6-9]\d{9}\b"),
];

/// One compiled expression: its kind, its replacement and the regex.
struct Pattern {
    kind: &'static str,
    marker: String,
    regex: Regex,
}

/// A UUID, whose all-digit groups look like an Aadhaar number.
static UUID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b")
        .expect("the UUID pattern is valid")
});

static COMPILED: LazyLock<Vec<Pattern>> = LazyLock::new(|| {
    PATTERNS
        .iter()
        .map(|&(kind, pattern)| Pattern {
            kind,
            marker: format!("[redacted:{kind}]"),
            regex: Regex::new(pattern).expect("the redaction patterns are valid"),
        })
        .collect()
});

/// Redacts `raw`: every expression, in order, with `replace_all`.
pub fn redact(raw: &str) -> Redacted {
    let mut text = raw.to_owned();
    for pattern in COMPILED.iter() {
        let replaced = if pattern.kind == "kv" {
            pattern.regex.replace_all(&text, |captures: &Captures| {
                if secret_shaped(&captures[2]) {
                    pattern.marker.clone()
                } else {
                    captures[0].to_owned()
                }
            })
        } else if matches!(pattern.kind, "aadhaar" | "phone") {
            let uuids: Vec<(usize, usize)> = UUID
                .find_iter(&text)
                .map(|m| (m.start(), m.end()))
                .collect();
            let inside = |s: usize, e: usize| uuids.iter().any(|&(a, b)| a <= s && e <= b);
            pattern.regex.replace_all(&text, |captures: &Captures| {
                let m = captures.get(0).expect("group 0 is the match");
                if inside(m.start(), m.end()) {
                    m.as_str().to_owned()
                } else {
                    pattern.marker.clone()
                }
            })
        } else {
            pattern.regex.replace_all(&text, NoExpand(&pattern.marker))
        };
        if let Cow::Owned(changed) = replaced {
            text = changed;
        }
    }
    Redacted(text)
}

/// A `kv` value that holds both a letter and a digit.
fn secret_shaped(value: &str) -> bool {
    value.bytes().any(|b| b.is_ascii_alphabetic()) && value.bytes().any(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/redaction");

    fn corpus(name: &str) -> String {
        fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap()
    }

    #[test]
    fn leaks_redact_to_expected() {
        let leaks = corpus("leaks.txt");
        let expected = corpus("expected.txt");
        let leak_lines: Vec<&str> = leaks.lines().collect();
        let expected_lines: Vec<&str> = expected.lines().collect();
        // One line per expression, in table order, then two kv secrets in
        // JSON: inside a string with its quotes escaped, and as a quoted key
        // and value.
        assert_eq!(leak_lines.len(), PATTERNS.len() + 2);
        assert_eq!(expected_lines.len(), leak_lines.len());
        for (i, pattern) in COMPILED.iter().enumerate() {
            let (leak, want) = (leak_lines[i], expected_lines[i]);
            assert!(
                pattern.regex.is_match(leak),
                "line {} is not {}",
                i + 1,
                pattern.kind
            );
            assert!(want.contains(&pattern.marker), "line {}: {want}", i + 1);
        }
        for (i, (leak, want)) in leak_lines.iter().zip(&expected_lines).enumerate() {
            assert_eq!(redact(leak).as_str(), *want, "line {}", i + 1);
        }
        assert_eq!(redact(&leaks).as_str(), expected);
    }

    #[test]
    fn code_redacts_to_itself() {
        let code = corpus("code.txt");
        for line in [
            "token: string",
            "tokens: 15",
            "const apiKey = process.env.API_KEY",
            "headers.get(\"Authorization\")",
        ] {
            assert!(code.contains(line), "code.txt lacks {line}");
        }
        let ten_digits = Regex::new(r"\b[0-5]\d{9}\b").unwrap();
        assert!(
            ten_digits.is_match(&code),
            "code.txt lacks a 10-digit number from 0 to 5"
        );
        assert_eq!(redact(&code).as_str(), code);
    }

    #[test]
    fn uuids_and_package_specifiers_stay_as_written() {
        for text in [
            "device FF785D94-2345-6789-4012-41D2146ED797 booted",
            "session efbb4580-2234-5678-9012-0da3a0272027 resumed",
            "fetched https://unpkg.com/@expo/ui@0.2.0/ios/GlassView.swift",
            "npm i react@18.3.1 @types/node@20.10.5",
        ] {
            assert_eq!(redact(text).as_str(), text);
        }
        // The same digits outside a UUID, and a real address, still go.
        assert_eq!(
            redact("id 2345 6789 4012 here").as_str(),
            "id [redacted:aadhaar] here"
        );
        assert_eq!(
            redact("mail a.b+c@mail.example.co.uk now").as_str(),
            "mail [redacted:email] now"
        );
    }

    #[test]
    fn kv_redacts_values_with_a_letter_and_a_digit_only() {
        let letters = "api_key = abcdefghijklmnopqrst";
        assert_eq!(redact(letters).as_str(), letters);
        let digits = "api_key = 11111111111111111111";
        assert_eq!(redact(digits).as_str(), digits);
        let both = "the api_key = abcdefghij0123456789 here";
        assert_eq!(redact(both).as_str(), "the [redacted:kv] here");
        // A quote right before the key is part of the match, even when it
        // opens the JSON string the key starts.
        let escaped = r#"{"cmd":"TOKEN=\"abcdefghij0123456789\" run"}"#;
        assert_eq!(redact(escaped).as_str(), r#"{"cmd":[redacted:kv] run"}"#);
        let keyed = r#"{"id": 7, "api_key": "abcdefghij0123456789"}"#;
        assert_eq!(redact(keyed).as_str(), r#"{"id": 7, [redacted:kv]}"#);
        let nested = r#"{"body":"{\"token\": \"abcdefghij0123456789\"}"}"#;
        assert_eq!(redact(nested).as_str(), r#"{"body":"{[redacted:kv]}"}"#);
        let unquoted_type = "type Session = { token: string; tokens: 15 };";
        assert_eq!(redact(unquoted_type).as_str(), unquoted_type);
    }
}

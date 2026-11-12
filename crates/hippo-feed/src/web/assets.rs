//! The stylesheets and the icon, served from the `hippo-theme` crate.

use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// Lower-case hex of the SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest.as_slice() {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// The `?v=` values of the static files: the first 12 hex digits of each
/// file's SHA-256, computed once at start. The server serves the current
/// file whatever the value; it only changes the URL.
#[derive(Debug, Clone)]
pub struct Assets {
    base: String,
    dark: String,
    auto: String,
    icon: String,
}

impl Assets {
    pub fn new() -> Self {
        let short = |text: &str| sha256_hex(text.as_bytes())[..12].to_string();
        Self {
            base: short(hippo_theme::BASE_CSS),
            dark: short(hippo_theme::DARK_CSS),
            auto: short(hippo_theme::AUTO_CSS),
            icon: short(hippo_theme::ICON_SVG),
        }
    }

    pub fn base_href(&self) -> String {
        format!("/static/base.css?v={}", self.base)
    }

    pub fn dark_href(&self) -> String {
        format!("/static/themes/dark.css?v={}", self.dark)
    }

    pub fn auto_href(&self) -> String {
        format!("/static/themes/auto.css?v={}", self.auto)
    }

    pub fn icon_href(&self) -> String {
        format!("/static/icon.svg?v={}", self.icon)
    }
}

impl Default for Assets {
    fn default() -> Self {
        Self::new()
    }
}

fn immutable(content_type: &'static str, body: &'static str) -> Response {
    let mut response = body.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(IMMUTABLE));
    response
}

const CSS: &str = "text/css; charset=utf-8";

pub async fn base_css() -> Response {
    immutable(CSS, hippo_theme::BASE_CSS)
}

pub async fn dark_css() -> Response {
    immutable(CSS, hippo_theme::DARK_CSS)
}

pub async fn auto_css() -> Response {
    immutable(CSS, hippo_theme::AUTO_CSS)
}

pub async fn icon_svg() -> Response {
    immutable("image/svg+xml", hippo_theme::ICON_SVG)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_is_lower_case_and_64_long() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn hrefs_carry_twelve_hex_digits() {
        let a = Assets::new();
        for href in [a.base_href(), a.dark_href(), a.auto_href(), a.icon_href()] {
            let v = href.split("?v=").nth(1).expect("v");
            assert_eq!(v.len(), 12);
            assert!(v
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        }
        assert!(a.base_href().starts_with("/static/base.css?v="));
    }
}

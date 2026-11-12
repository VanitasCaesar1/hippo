//! Markdown for posts and comments, rendered with `pulldown-cmark` and
//! restricted.
//!
//! The parser's events are rewritten before `push_html`:
//!
//! - raw HTML, block or inline, becomes text, so it shows as escaped text;
//! - a link, reference link or autolink whose destination parses with `url`
//!   as `http` or `https` with a host, or as `mailto`, becomes
//!   `<a href="<the parsed URL>" rel="nofollow ugc noopener noreferrer">`
//!   and is counted; any other link is written as its text alone;
//! - an image becomes a link to its source under the same rule, with its alt
//!   text as the link's text, so no page loads a remote image;
//! - a heading becomes a paragraph of bold text;
//! - tables, footnotes and every other extension stay off.
//!
//! An anchor never nests: a link or image inside a link that is being written
//! is written as its text alone, and counted for nothing.

use pulldown_cmark::{html, CowStr, Event, LinkType, Options, Parser, Tag, TagEnd};
use url::Url;

const REL: &str = "nofollow ugc noopener noreferrer";

/// What to do with the end of a link or image that opened.
#[derive(Clone, Copy)]
enum Opened {
    /// An anchor was written; close it.
    Anchor,
    /// Nothing was written; write nothing.
    Text,
}

/// The URL as `url` writes it, when the feed may link to it.
fn allowed(destination: &str, link_type: LinkType) -> Option<String> {
    let destination = if link_type == LinkType::Email {
        format!("mailto:{destination}")
    } else {
        destination.to_owned()
    };
    let url = Url::parse(&destination).ok()?;
    match url.scheme() {
        "http" | "https" if url.host().is_some() => Some(url.into()),
        "mailto" => Some(url.into()),
        _ => None,
    }
}

/// Escapes `&`, `<`, `>`, `"` and `'` for text and attribute values.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

fn raw(html: impl Into<CowStr<'static>>) -> Event<'static> {
    Event::Html(html.into())
}

/// Renders `text` to HTML, and counts the links written.
pub fn render(text: &str) -> (String, usize) {
    let mut stack: Vec<Opened> = Vec::new();
    let mut inside_anchor = false;
    let mut links = 0usize;

    let events = Parser::new_ext(text, Options::empty()).flat_map(|event| -> Vec<Event<'_>> {
        match event {
            Event::Html(s) | Event::InlineHtml(s) => vec![Event::Text(s)],
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                ..
            })
            | Event::Start(Tag::Image {
                link_type,
                dest_url,
                ..
            }) => {
                let url = if inside_anchor {
                    None
                } else {
                    allowed(&dest_url, link_type)
                };
                match url {
                    Some(url) => {
                        links += 1;
                        inside_anchor = true;
                        stack.push(Opened::Anchor);
                        vec![raw(format!("<a href=\"{}\" rel=\"{REL}\">", escape(&url)))]
                    }
                    None => {
                        stack.push(Opened::Text);
                        vec![]
                    }
                }
            }
            Event::End(TagEnd::Link | TagEnd::Image) => match stack.pop() {
                Some(Opened::Anchor) => {
                    inside_anchor = false;
                    vec![raw("</a>")]
                }
                _ => vec![],
            },
            Event::Start(Tag::Heading { .. }) => {
                vec![Event::Start(Tag::Paragraph), raw("<strong>")]
            }
            Event::End(TagEnd::Heading(_)) => {
                vec![raw("</strong>"), Event::End(TagEnd::Paragraph)]
            }
            other => vec![other],
        }
    });

    let mut out = String::new();
    html::push_html(&mut out, events);
    (out, links)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is(text: &str, html: &str, links: usize) {
        assert_eq!(render(text), (html.to_owned(), links), "{text:?}");
    }

    const A: &str = r#"rel="nofollow ugc noopener noreferrer""#;

    #[test]
    fn raw_html_shows_as_escaped_text() {
        is(
            "<script>alert(1)</script>",
            "&lt;script&gt;alert(1)&lt;/script&gt;",
            0,
        );
        is(
            "<img src=x onerror=alert(1)>",
            "&lt;img src=x onerror=alert(1)&gt;",
            0,
        );
        is("<!-- c -->", "&lt;!-- c --&gt;", 0);
    }

    #[test]
    fn inline_html_shows_as_escaped_text() {
        is(
            "a <b onclick=\"x\">b</b> c",
            "<p>a &lt;b onclick=\"x\"&gt;b&lt;/b&gt; c</p>\n",
            0,
        );
        is(
            "a <script>alert(1)</script>",
            "<p>a &lt;script&gt;alert(1)&lt;/script&gt;</p>\n",
            0,
        );
    }

    #[test]
    fn unsafe_link_targets_leave_only_the_text() {
        is("[x](javascript:alert(1))", "<p>x</p>\n", 0);
        is("[x](data:text/html,y)", "<p>x</p>\n", 0);
        is("[x](JaVaScRiPt:alert(1))", "<p>x</p>\n", 0);
        is("[x](  javascript:alert(1))", "<p>x</p>\n", 0);
        is(
            "[x](java\tscript:alert(1))",
            "<p>[x](java\tscript:alert(1))</p>\n",
            0,
        );
        is("[x](vbscript:x)", "<p>x</p>\n", 0);
        is("[x](file:///etc/passwd)", "<p>x</p>\n", 0);
        is("[x](ftp://example.com/f)", "<p>x</p>\n", 0);
        is("[x](/relative)", "<p>x</p>\n", 0);
        is("[x](//evil.example/x)", "<p>x</p>\n", 0);
        is("[x](http://)", "<p>x</p>\n", 0);
        is("[x](&#106;avascript:alert(1))", "<p>x</p>\n", 0);
    }

    #[test]
    fn a_raw_anchor_is_text_not_a_link() {
        is(
            "<a href=\"javascript:x\">y</a>",
            "<p>&lt;a href=\"javascript:x\"&gt;y&lt;/a&gt;</p>\n",
            0,
        );
        is(
            "see <a href=\"javascript:x\">y</a>",
            "<p>see &lt;a href=\"javascript:x\"&gt;y&lt;/a&gt;</p>\n",
            0,
        );
    }

    #[test]
    fn an_autolink_to_a_script_is_text() {
        is("<javascript:alert(1)>", "<p>javascript:alert(1)</p>\n", 0);
        is("<data:text/html,y>", "<p>data:text/html,y</p>\n", 0);
    }

    #[test]
    fn a_reference_link_follows_the_same_rule() {
        is("[x][r]\n\n[r]: javascript:alert(1)", "<p>x</p>\n", 0);
        is(
            "[x][r]\n\n[r]: https://e.example/",
            &format!("<p><a href=\"https://e.example/\" {A}>x</a></p>\n"),
            1,
        );
        is("[x][missing]", "<p>[x][missing]</p>\n", 0);
    }

    #[test]
    fn a_link_is_written_in_its_parsed_form() {
        is(
            "[x](http:/evil.example)",
            &format!("<p><a href=\"http://evil.example/\" {A}>x</a></p>\n"),
            1,
        );
        is(
            "[x](HTTPS://Example.COM/A b)",
            "<p>[x](HTTPS://Example.COM/A b)</p>\n",
            0,
        );
        is(
            "[x](HTTPS://Example.COM/A)",
            &format!("<p><a href=\"https://example.com/A\" {A}>x</a></p>\n"),
            1,
        );
    }

    #[test]
    fn a_target_is_escaped_in_the_attribute() {
        is(
            "[x](https://e.example/?a=1&b='2')",
            &format!("<p><a href=\"https://e.example/?a=1&amp;b=%272%27\" {A}>x</a></p>\n"),
            1,
        );
        is(
            "[x](https://e.example/\"onmouseover=\"alert(1))",
            &format!("<p><a href=\"https://e.example/%22onmouseover=%22alert(1)\" {A}>x</a></p>\n"),
            1,
        );
        let (html, links) = render("[x](<https://e.example/\"><script>>)");
        assert_eq!(links, 1, "{html}");
        assert!(!html.contains("<script>"), "{html}");
        assert!(!html.contains("\"><"), "{html}");
    }

    #[test]
    fn escape_covers_the_five_characters() {
        assert_eq!(escape("a&b<c>d\"e'f"), "a&amp;b&lt;c&gt;d&quot;e&#39;f");
    }

    #[test]
    fn an_image_becomes_a_link_with_its_alt_text() {
        is(
            "![a](https://e.example/i.png)",
            &format!("<p><a href=\"https://e.example/i.png\" {A}>a</a></p>\n"),
            1,
        );
        is("![a](javascript:alert(1))", "<p>a</p>\n", 0);
        is("![a](data:image/png;base64,AAAA)", "<p>a</p>\n", 0);
        is(
            "![](https://e.example/i.png)",
            &format!("<p><a href=\"https://e.example/i.png\" {A}></a></p>\n"),
            1,
        );
    }

    #[test]
    fn an_image_never_loads() {
        let (html, _) = render("![a](https://e.example/i.png) ![b](http://e.example/j.gif)");
        assert!(!html.contains("<img"), "{html}");
    }

    #[test]
    fn an_image_inside_a_link_does_not_nest_anchors() {
        is(
            "[![a](https://i.example/x.png)](https://l.example/)",
            &format!("<p><a href=\"https://l.example/\" {A}>a</a></p>\n"),
            1,
        );
        is(
            "[![a](https://i.example/x.png)](javascript:alert(1))",
            &format!("<p><a href=\"https://i.example/x.png\" {A}>a</a></p>\n"),
            1,
        );
        is("[![a](javascript:x)](javascript:y)", "<p>a</p>\n", 0);
    }

    #[test]
    fn an_email_autolink_gets_mailto() {
        is(
            "<ada@example.com>",
            &format!("<p><a href=\"mailto:ada@example.com\" {A}>ada@example.com</a></p>\n"),
            1,
        );
    }

    #[test]
    fn a_web_autolink_is_a_link() {
        is(
            "<https://e.example/x>",
            &format!("<p><a href=\"https://e.example/x\" {A}>https://e.example/x</a></p>\n"),
            1,
        );
    }

    #[test]
    fn a_bare_url_is_plain_text() {
        is(
            "see https://e.example/x",
            "<p>see https://e.example/x</p>\n",
            0,
        );
    }

    #[test]
    fn a_mailto_link_is_kept() {
        is(
            "[mail](mailto:ada@example.com)",
            &format!("<p><a href=\"mailto:ada@example.com\" {A}>mail</a></p>\n"),
            1,
        );
    }

    #[test]
    fn headings_become_bold_paragraphs() {
        is("# h", "<p><strong>h</strong></p>\n", 0);
        is("###### six", "<p><strong>six</strong></p>\n", 0);
        is("Title\n=====", "<p><strong>Title</strong></p>\n", 0);
        is("Sub\n---", "<p><strong>Sub</strong></p>\n", 0);
        is("# a *b*", "<p><strong>a <em>b</em></strong></p>\n", 0);
        let (html, _) = render("# a\n## b\n### c");
        assert!(!html.contains("<h"), "{html}");
    }

    #[test]
    fn a_link_in_a_heading_is_counted() {
        is(
            "# [x](https://e.example/)",
            &format!("<p><strong><a href=\"https://e.example/\" {A}>x</a></strong></p>\n"),
            1,
        );
    }

    #[test]
    fn tables_and_footnotes_are_off() {
        is(
            "| a | b |\n|---|---|\n| 1 | 2 |",
            "<p>| a | b |\n|---|---|\n| 1 | 2 |</p>\n",
            0,
        );
        // With footnotes off, `[^1]: note` is an ordinary link reference whose
        // target is relative, so it renders as its text.
        is("a[^1]\n\n[^1]: note", "<p>a^1</p>\n", 0);
    }

    #[test]
    fn ordinary_markdown_renders() {
        is(
            "a *b* **c** `d`",
            "<p>a <em>b</em> <strong>c</strong> <code>d</code></p>\n",
            0,
        );
        is("- a\n- b", "<ul>\n<li>a</li>\n<li>b</li>\n</ul>\n", 0);
        is("> q", "<blockquote>\n<p>q</p>\n</blockquote>\n", 0);
        is(
            "```\n<script>x</script>\n```",
            "<pre><code>&lt;script&gt;x&lt;/script&gt;\n</code></pre>\n",
            0,
        );
        is("", "", 0);
    }

    #[test]
    fn links_are_counted() {
        let (html, links) = render(
            "[a](https://a.example/) [b](http://b.example/) <c@d.example> [e](javascript:x)",
        );
        assert_eq!(links, 3, "{html}");
        assert_eq!(html.matches("<a ").count(), 3);
    }

    #[test]
    fn no_output_carries_an_active_script_vector() {
        let corpus = [
            "<script>alert(1)</script>",
            "<img src=x onerror=alert(1)>",
            "<svg onload=alert(1)>",
            "<iframe src=javascript:alert(1)></iframe>",
            "[x](javascript:alert(1))",
            "[x](JaVaScRiPt:alert(1))",
            "[x]( javascript:alert(1))",
            "[x](&#x6A;avascript:alert(1))",
            "[x](java&#10;script:alert(1))",
            "<javascript:alert(1)>",
            "[x][r]\n\n[r]: javascript:alert(1)",
            "![x](javascript:alert(1))",
            "![x](x\" onerror=\"alert(1))",
            "[![x](javascript:alert(1))](javascript:alert(2))",
            "<a href=\"javascript:alert(1)\">x</a>",
            "<a href=javascript:alert(1)>x</a>",
            "[x](data:text/html;base64,PHNjcmlwdD4=)",
            "[x](https://e.example/\" onclick=\"alert(1))",
            "# <script>alert(1)</script>",
            "`<script>`<script>alert(1)</script>",
            "<div style=\"x\">y</div>",
        ];
        for text in corpus {
            let (html, _) = render(text);
            // Every `<` that opens a tag is one of the allowed tags.
            for (i, _) in html.match_indices('<') {
                let rest = &html[i + 1..];
                let ok = [
                    "a ",
                    "/a>",
                    "p>",
                    "/p>",
                    "strong>",
                    "/strong>",
                    "em>",
                    "/em>",
                    "code>",
                    "/code>",
                    "pre>",
                    "/pre>",
                    "ul>",
                    "/ul>",
                    "li>",
                    "/li>",
                    "blockquote>",
                    "/blockquote>",
                ]
                .iter()
                .any(|t| rest.starts_with(t));
                assert!(ok, "{text:?} -> {html}");
            }
            // Every href is an http(s) or mailto URL.
            for (i, _) in html.match_indices("<a href=\"") {
                let value = &html[i + 9..];
                assert!(
                    value.starts_with("http://")
                        || value.starts_with("https://")
                        || value.starts_with("mailto:"),
                    "{text:?} -> {html}"
                );
            }
        }
    }
}

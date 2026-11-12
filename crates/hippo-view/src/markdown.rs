//! A small Markdown reader for the session tab and note bodies: the block
//! and inline subset agents and people write in chat, turned into blocks of
//! styled spans. No HTML, no nesting beyond list depth, no reference links.

/// A run of inline text with one style.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    /// A web link's target.
    pub link: Option<String>,
}

/// One block of a Markdown text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// `#` to `######`.
    Heading {
        level: u8,
        spans: Vec<Span>,
    },
    /// Lines of text; a line break inside stays a line break.
    Paragraph(Vec<Span>),
    /// A list item: its marker (`•`, or `3.`) and depth from 0.
    Item {
        marker: String,
        depth: usize,
        spans: Vec<Span>,
    },
    Quote(Vec<Span>),
    /// A fenced code block, or a table, as monospace text.
    Code {
        lang: Option<String>,
        text: String,
    },
    Rule,
}

/// The blocks of `text`.
pub fn parse(text: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    let mut para: Vec<&str> = Vec::new();
    let flush = |para: &mut Vec<&str>, out: &mut Vec<Block>| {
        if !para.is_empty() {
            out.push(Block::Paragraph(inline(&para.join("\n"))));
            para.clear();
        }
    };
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        // Fenced code.
        if let Some(fence) = fence_of(trimmed) {
            flush(&mut para, &mut out);
            let lang = trimmed[fence.len()..].trim();
            let lang = (!lang.is_empty()).then(|| lang.to_string());
            let mut body = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with(fence) {
                body.push(lines[i]);
                i += 1;
            }
            i += 1; // the closing fence, or past the end
            out.push(Block::Code {
                lang,
                text: body.join("\n"),
            });
            continue;
        }
        if trimmed.is_empty() {
            flush(&mut para, &mut out);
            i += 1;
            continue;
        }
        if let Some((level, rest)) = heading(trimmed) {
            flush(&mut para, &mut out);
            out.push(Block::Heading {
                level,
                spans: inline(rest),
            });
            i += 1;
            continue;
        }
        if is_rule(trimmed) {
            flush(&mut para, &mut out);
            out.push(Block::Rule);
            i += 1;
            continue;
        }
        if trimmed.starts_with('|') {
            flush(&mut para, &mut out);
            let mut rows = Vec::new();
            while i < lines.len() && lines[i].trim_start().starts_with('|') {
                rows.push(lines[i].trim());
                i += 1;
            }
            out.push(Block::Code {
                lang: None,
                text: rows.join("\n"),
            });
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('>') {
            flush(&mut para, &mut out);
            let mut quoted = vec![rest.strip_prefix(' ').unwrap_or(rest)];
            i += 1;
            while i < lines.len() {
                match lines[i].trim_start().strip_prefix('>') {
                    Some(r) => quoted.push(r.strip_prefix(' ').unwrap_or(r)),
                    None => break,
                }
                i += 1;
            }
            out.push(Block::Quote(inline(&quoted.join("\n"))));
            continue;
        }
        if let Some((marker, rest)) = list_marker(trimmed) {
            flush(&mut para, &mut out);
            let indent = line.len() - trimmed.len();
            let mut body = vec![rest];
            i += 1;
            // Continuation lines: indented, not blank, not a new item.
            while i < lines.len() {
                let next = lines[i];
                let nt = next.trim_start();
                let next_indent = next.len() - nt.len();
                if nt.is_empty()
                    || next_indent <= indent
                    || list_marker(nt).is_some()
                    || fence_of(nt).is_some()
                {
                    break;
                }
                body.push(nt);
                i += 1;
            }
            out.push(Block::Item {
                marker,
                depth: indent / 2,
                spans: inline(&body.join("\n")),
            });
            continue;
        }
        para.push(line);
        i += 1;
    }
    flush(&mut para, &mut out);
    out
}

fn fence_of(line: &str) -> Option<&'static str> {
    if line.starts_with("```") {
        Some("```")
    } else if line.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

fn heading(line: &str) -> Option<(u8, &str)> {
    let hashes = line.bytes().take_while(|b| *b == b'#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &line[hashes..];
    if rest.is_empty() {
        return Some((hashes as u8, ""));
    }
    rest.strip_prefix(' ')
        .map(|r| (hashes as u8, r.trim().trim_end_matches('#').trim_end()))
}

fn is_rule(line: &str) -> bool {
    let t: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    t.len() >= 3
        && (t.chars().all(|c| c == '-')
            || t.chars().all(|c| c == '*')
            || t.chars().all(|c| c == '_'))
}

/// `- `, `* `, `+ ` (shown as `•`) or `12. `, `12) ` (shown as `12.`).
fn list_marker(line: &str) -> Option<(String, &str)> {
    for bullet in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(bullet) {
            return Some(("•".to_string(), rest));
        }
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if (1..=9).contains(&digits) {
        let rest = &line[digits..];
        if let Some(r) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some((format!("{}.", &line[..digits]), r));
        }
    }
    None
}

/// The spans of one block's text: `code`, **bold**, *italic*,
/// [label](https://…) and bare web addresses.
pub fn inline(text: &str) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    let mut plain = String::new();
    let mut bold = false;
    let mut italic = false;
    let push = |out: &mut Vec<Span>,
                text: &str,
                bold: bool,
                italic: bool,
                code: bool,
                link: Option<String>| {
        if text.is_empty() {
            return;
        }
        if let Some(last) = out.last_mut() {
            if last.bold == bold
                && last.italic == italic
                && !last.code
                && !code
                && last.link.is_none()
                && link.is_none()
            {
                last.text.push_str(text);
                return;
            }
        }
        out.push(Span {
            text: text.to_string(),
            bold,
            italic,
            code,
            link,
        });
    };
    let mut i = 0;
    let bytes = text.as_bytes();
    while i < text.len() {
        let rest = &text[i..];
        // Code: a run of backticks up to the same run.
        if rest.starts_with('`') {
            let ticks = rest.bytes().take_while(|b| *b == b'`').count();
            let fence = &rest[..ticks];
            if let Some(end) = rest[ticks..].find(fence) {
                push(&mut out, &plain, bold, italic, false, None);
                plain.clear();
                let code = &rest[ticks..ticks + end];
                let code = code
                    .strip_prefix(' ')
                    .and_then(|c| c.strip_suffix(' '))
                    .unwrap_or(code);
                push(&mut out, code, bold, italic, true, None);
                i += ticks + end + ticks;
                continue;
            }
        }
        if let Some(after) = rest.strip_prefix("**") {
            if bold || after.contains("**") {
                push(&mut out, &plain, bold, italic, false, None);
                plain.clear();
                bold = !bold;
                i += 2;
                continue;
            }
        }
        if rest.starts_with('*') && !rest.starts_with("**") {
            let next = rest[1..].chars().next();
            let opens =
                !italic && next.is_some_and(|c| !c.is_whitespace()) && rest[1..].contains('*');
            let prev_ok = i == 0 || !bytes[i - 1].is_ascii_alphanumeric() || italic;
            if (opens && prev_ok) || italic {
                push(&mut out, &plain, bold, italic, false, None);
                plain.clear();
                italic = !italic;
                i += 1;
                continue;
            }
        }
        if rest.starts_with('[') {
            if let Some((label, url, len)) = markdown_link(rest) {
                push(&mut out, &plain, bold, italic, false, None);
                plain.clear();
                push(
                    &mut out,
                    label,
                    bold,
                    italic,
                    false,
                    followable(url).then(|| url.to_string()),
                );
                i += len;
                continue;
            }
        }
        if (rest.starts_with("https://") || rest.starts_with("http://"))
            && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
        {
            let end = rest
                .find(|c: char| c.is_whitespace() || c == '<' || c == '>' || c == '"' || c == '`')
                .unwrap_or(rest.len());
            let url = rest[..end].trim_end_matches(['.', ',', ';', ':', ')', '!', '?', '\'']);
            push(&mut out, &plain, bold, italic, false, None);
            plain.clear();
            push(&mut out, url, bold, italic, false, Some(url.to_string()));
            i += url.len();
            continue;
        }
        let c = rest.chars().next().unwrap_or(' ');
        plain.push(c);
        i += c.len_utf8();
    }
    push(&mut out, &plain, bold, italic, false, None);
    out
}

/// Whether a link's target is one the app follows: a web address, or a
/// note link the note tab writes (`hippo-note:<n>`, `hippo-dangling:<n>`).
/// Others, such as a client's citation scheme, show as their label only.
pub fn followable(url: &str) -> bool {
    [
        "http://",
        "https://",
        "mailto:",
        "hippo-note:",
        "hippo-dangling:",
    ]
    .iter()
    .any(|scheme| url.starts_with(scheme))
}

/// `[label](url)` at the start of `text`: the label, the url and the length.
fn markdown_link(text: &str) -> Option<(&str, &str, usize)> {
    let close = text.find("](")?;
    let label = &text[1..close];
    if label.contains('\n') || label.contains('[') {
        return None;
    }
    let rest = &text[close + 2..];
    let end = rest.find(')')?;
    let url = &rest[..end];
    if url.contains(char::is_whitespace) {
        return None;
    }
    Some((label, url, close + 2 + end + 1))
}

/// The text of a block's spans, as drawn.
pub fn plain(spans: &[Span]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}

/// `text` cut to at most `max_lines` lines and `max_chars` characters, at a
/// line end where one comes near enough; and whether anything was cut.
pub fn clip(text: &str, max_lines: usize, max_chars: usize) -> (&str, bool) {
    let mut end = text.len();
    let mut cut = false;
    if let Some((i, _)) = text.match_indices('\n').nth(max_lines.saturating_sub(1)) {
        end = i;
        cut = true;
    }
    if let Some((i, _)) = text.char_indices().nth(max_chars) {
        if i < end {
            // Back to the last line end when it is not far.
            end = match text[..i].rfind('\n') {
                Some(nl) if nl > i / 2 => nl,
                _ => i,
            };
            cut = true;
        }
    }
    (text[..end].trim_end(), cut)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(text: &str) -> Span {
        Span {
            text: text.into(),
            ..Span::default()
        }
    }

    #[test]
    fn blocks_of_a_chat_reply() {
        let text = "# Plan\n\nFirst line\nsecond line\n\n- one\n  more of one\n- two\n  1. nested\n\n```rust\nfn main() {}\n```\n> quoted\n> text\n\n| a | b |\n|---|---|\n\n---\nend";
        let blocks = parse(text);
        assert_eq!(
            blocks[0],
            Block::Heading {
                level: 1,
                spans: vec![span("Plan")]
            }
        );
        assert_eq!(
            blocks[1],
            Block::Paragraph(vec![span("First line\nsecond line")])
        );
        assert_eq!(
            blocks[2],
            Block::Item {
                marker: "•".into(),
                depth: 0,
                spans: vec![span("one\nmore of one")]
            }
        );
        assert!(matches!(&blocks[4], Block::Item { marker, depth: 1, .. } if marker == "1."));
        assert_eq!(
            blocks[5],
            Block::Code {
                lang: Some("rust".into()),
                text: "fn main() {}".into()
            }
        );
        assert_eq!(blocks[6], Block::Quote(vec![span("quoted\ntext")]));
        assert!(
            matches!(&blocks[7], Block::Code { lang: None, text } if text.starts_with("| a | b |"))
        );
        assert_eq!(blocks[8], Block::Rule);
        assert_eq!(blocks[9], Block::Paragraph(vec![span("end")]));
    }

    #[test]
    fn an_unclosed_fence_runs_to_the_end() {
        let blocks = parse("text\n```\ncode\nmore");
        assert_eq!(blocks.len(), 2);
        assert!(matches!(&blocks[1], Block::Code { text, .. } if text == "code\nmore"));
    }

    #[test]
    fn inline_styles_links_and_code() {
        let spans = inline("Run `cargo test` **now**, see [docs](https://example.com/a) or https://x.dev/b. snake_case *stays* a_b");
        let texts: Vec<(&str, bool, bool, bool, Option<&str>)> = spans
            .iter()
            .map(|s| (s.text.as_str(), s.bold, s.italic, s.code, s.link.as_deref()))
            .collect();
        assert_eq!(
            texts,
            vec![
                ("Run ", false, false, false, None),
                ("cargo test", false, false, true, None),
                (" ", false, false, false, None),
                ("now", true, false, false, None),
                (", see ", false, false, false, None),
                ("docs", false, false, false, Some("https://example.com/a")),
                (" or ", false, false, false, None),
                (
                    "https://x.dev/b",
                    false,
                    false,
                    false,
                    Some("https://x.dev/b")
                ),
                (". snake_case ", false, false, false, None),
                ("stays", false, true, false, None),
                (" a_b", false, false, false, None),
            ]
        );
        // A citation link shows its label only; no target.
        let cited = inline("[Assistant quote](t3-citation://v1/a?b=c) ok");
        assert_eq!(cited[0].text, "Assistant quote ok");
        assert_eq!(cited[0].link, None);
        let note = inline("see [Deploy](hippo-note:3)");
        assert_eq!(note[1].link.as_deref(), Some("hippo-note:3"));
        // Lone markers stay text.
        assert_eq!(plain(&inline("2 * 3 and a ** b")), "2 * 3 and a ** b");
    }

    #[test]
    fn clipping_prefers_a_line_end() {
        let text = "a\nb\nc\nd";
        assert_eq!(clip(text, 2, 100), ("a\nb", true));
        assert_eq!(clip(text, 10, 100), (text, false));
        let long = format!("{}\n{}", "x".repeat(60), "y".repeat(60));
        assert_eq!(clip(&long, 10, 100).0, "x".repeat(60));
        assert_eq!(clip(&"z".repeat(50), 10, 20), (&*"z".repeat(20), true));
    }
}

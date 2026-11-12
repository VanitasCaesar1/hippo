//! Dreaming step 2, notes over budget: a note too large for
//! a prompt is read in parts made of whole sections.
//!
//! A section starts at each heading line (one to six `#` and a space, as in
//! Conventions) outside a fenced code block (a line starting with three
//! backticks or three tildes opens or closes one); the text before the first
//! heading is a section of its own. A part is a run of consecutive
//! sections of at most a given size; a section larger than that is cut at
//! line ends, and a line larger than that is a part alone.

use std::ops::Range;

/// An excerpt of a note: a byte range of its bytes, and its lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Excerpt {
    pub bytes: Range<usize>,
    /// The first and last line shown, counted from 1.
    pub first_line: usize,
    pub last_line: usize,
    /// The note's line count.
    pub lines: usize,
}

impl Excerpt {
    /// The excerpt of `bytes` at `range`.
    pub fn of(bytes: &[u8], range: Range<usize>) -> Excerpt {
        let lines_before = |end: usize| bytes[..end].iter().filter(|&&b| b == b'\n').count();
        let shown = &bytes[range.clone()];
        let first_line = lines_before(range.start) + 1;
        let inner = shown.iter().filter(|&&b| b == b'\n').count();
        let last_line = if shown.ends_with(b"\n") || shown.is_empty() {
            first_line + inner.saturating_sub(1)
        } else {
            first_line + inner
        };
        Excerpt {
            bytes: range,
            first_line,
            last_line,
            lines: line_count(bytes),
        }
    }

    /// Whether the excerpt reaches the note's end.
    pub fn reaches_end(&self, note_len: usize) -> bool {
        self.bytes.end >= note_len
    }

    /// `lines <first>-<last> of <n>`.
    pub fn label(&self) -> String {
        format!(
            "lines {}-{} of {}",
            self.first_line, self.last_line, self.lines
        )
    }
}

fn line_count(bytes: &[u8]) -> usize {
    let newlines = bytes.iter().filter(|&&b| b == b'\n').count();
    if bytes.is_empty() || bytes.ends_with(b"\n") {
        newlines
    } else {
        newlines + 1
    }
}

/// The byte ranges of every line, newline included.
fn lines(bytes: &[u8]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < bytes.len() {
        let end = bytes[start..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(bytes.len(), |i| start + i + 1);
        out.push(start..end);
        start = end;
    }
    out
}

fn is_heading(line: &[u8]) -> bool {
    let hashes = line.iter().take_while(|&&b| b == b'#').count();
    (1..=6).contains(&hashes) && line.get(hashes) == Some(&b' ')
}

fn is_fence(line: &[u8]) -> bool {
    line.starts_with(b"```") || line.starts_with(b"~~~")
}

/// The byte ranges of the note's sections, in order, covering every byte.
pub fn sections(bytes: &[u8]) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut fenced = false;
    let mut start = 0;
    for line in lines(bytes) {
        let text = &bytes[line.clone()];
        if is_fence(text) {
            fenced = !fenced;
        } else if !fenced && is_heading(text) && line.start > start {
            out.push(start..line.start);
            start = line.start;
        }
    }
    if start < bytes.len() {
        out.push(start..bytes.len());
    }
    out
}

/// `range` cut at line ends into pieces of at most `max` bytes; a line over
/// `max` is a piece alone.
fn cut(bytes: &[u8], range: Range<usize>, max: usize) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = range.start;
    let mut end = range.start;
    for line in lines(&bytes[range.clone()]) {
        let line = (line.start + range.start)..(line.end + range.start);
        if line.end - start > max && end > start {
            out.push(start..end);
            start = end;
        }
        end = line.end;
    }
    if end > start {
        out.push(start..end);
    }
    out
}

/// The note's parts of at most `max` bytes each, in order, covering every
/// byte.
pub fn parts(bytes: &[u8], max: usize) -> Vec<Range<usize>> {
    let mut pieces = Vec::new();
    for section in sections(bytes) {
        if section.len() > max {
            pieces.extend(cut(bytes, section, max));
        } else {
            pieces.push(section);
        }
    }
    let mut out: Vec<Range<usize>> = Vec::new();
    for piece in pieces {
        match out.last_mut() {
            Some(last) if piece.end - last.start <= max => last.end = piece.end,
            _ => out.push(piece),
        }
    }
    out
}

/// The note's last sections that fit in `max` bytes together; when the last
/// section alone is larger, its last lines that fit (at least its last
/// line).
pub fn tail(bytes: &[u8], max: usize) -> Range<usize> {
    let all = sections(bytes);
    let Some(last) = all.last().cloned() else {
        return 0..0;
    };
    if last.len() > max {
        let mut start = last.end;
        for line in lines(&bytes[last.clone()]).iter().rev() {
            let line_start = line.start + last.start;
            if last.end - line_start > max && start < last.end {
                break;
            }
            start = line_start;
        }
        return start..last.end;
    }
    let mut start = last.start;
    for section in all.iter().rev().skip(1) {
        if last.end - section.start > max {
            break;
        }
        start = section.start;
    }
    start..last.end
}

/// Which parts of a note over budget a `per_note` pass reads, at most
/// `limit`, in note order: first the parts that hold text an observed edit
/// wrote (each text at least 16 bytes), then the last part.
pub fn chosen(
    bytes: &[u8],
    parts: &[Range<usize>],
    written: &[String],
    limit: usize,
) -> Vec<usize> {
    let mut picked: Vec<usize> = Vec::new();
    for text in written.iter().filter(|text| text.len() >= 16) {
        let needle = text.as_bytes();
        let Some(at) = bytes.windows(needle.len()).position(|w| w == needle) else {
            continue;
        };
        if let Some(i) = parts.iter().position(|part| part.contains(&at)) {
            if !picked.contains(&i) && picked.len() < limit {
                picked.push(i);
            }
        }
    }
    if let Some(last) = parts.len().checked_sub(1) {
        if !picked.contains(&last) && picked.len() < limit {
            picked.push(last);
        }
    }
    picked.sort_unstable();
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE: &str = "---\ntype: log\n---\n# Log\n\nIntro.\n\n## One\n\n```sh\n# not a heading\n```\n\nFirst entry.\n\n## Two\n\nSecond entry.\n";

    fn texts<'a>(bytes: &'a [u8], ranges: &[Range<usize>]) -> Vec<&'a str> {
        ranges
            .iter()
            .map(|r| std::str::from_utf8(&bytes[r.clone()]).unwrap())
            .collect()
    }

    #[test]
    fn sections_start_at_headings_outside_code_fences() {
        let bytes = NOTE.as_bytes();
        let found = sections(bytes);
        assert_eq!(
            texts(bytes, &found),
            [
                "---\ntype: log\n---\n",
                "# Log\n\nIntro.\n\n",
                "## One\n\n```sh\n# not a heading\n```\n\nFirst entry.\n\n",
                "## Two\n\nSecond entry.\n",
            ]
        );
        assert_eq!(found.iter().map(Range::len).sum::<usize>(), bytes.len());
    }

    #[test]
    fn parts_pack_whole_sections_and_cut_large_ones_at_lines() {
        let bytes = NOTE.as_bytes();
        let packed = parts(bytes, 40);
        assert_eq!(
            texts(bytes, &packed),
            [
                "---\ntype: log\n---\n# Log\n\nIntro.\n\n",
                "## One\n\n```sh\n# not a heading\n```\n\n",
                "First entry.\n\n## Two\n\nSecond entry.\n",
            ]
        );
        assert!(packed.iter().all(|p| p.len() <= 40));
        assert_eq!(parts(bytes, 10_000), vec![0..bytes.len()]);
    }

    #[test]
    fn the_tail_is_the_last_sections_that_fit() {
        let bytes = NOTE.as_bytes();
        assert_eq!(
            texts(bytes, &[tail(bytes, 30)]),
            ["## Two\n\nSecond entry.\n"]
        );
        let both = tail(bytes, 80);
        assert!(texts(bytes, &[both])[0].starts_with("## One"));
        // The last section alone is too large: its last lines that fit.
        assert_eq!(texts(bytes, &[tail(bytes, 16)]), ["\nSecond entry.\n"]);
    }

    #[test]
    fn an_excerpt_knows_its_lines() {
        let bytes = NOTE.as_bytes();
        let last = sections(bytes).pop().unwrap();
        let excerpt = Excerpt::of(bytes, last);
        assert_eq!(excerpt.label(), "lines 16-18 of 18");
        assert!(excerpt.reaches_end(bytes.len()));
        let first = Excerpt::of(bytes, 0..18);
        assert_eq!(first.label(), "lines 1-3 of 18");
        assert!(!first.reaches_end(bytes.len()));
    }

    #[test]
    fn edited_parts_come_first_then_the_last_part() {
        let bytes = NOTE.as_bytes();
        let packed = parts(bytes, 40);
        let written = vec!["# not a heading".to_owned() + "\n```", "short".to_owned()];
        assert_eq!(chosen(bytes, &packed, &written, 2), [1, 2]);
        assert_eq!(chosen(bytes, &packed, &[], 2), [2]);
        assert_eq!(chosen(bytes, &packed, &written, 1), [1]);
    }
}

//! Display-width helpers: tab expansion, control-character escaping,
//! column slicing and truncation (PLAN §6.4 "Display").

use bstr::ByteSlice;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// A run of display text; `special` runs are escapes like `^M`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seg {
    pub text: String,
    pub special: bool,
}

fn escape(ch: char) -> String {
    match ch as u32 {
        c @ 0..=0x1f => format!("^{}", char::from(c as u8 + 0x40)),
        0x7f => "^?".into(),
        c => format!("<U+{c:04X}>"),
    }
}

fn is_control(ch: char) -> bool {
    let c = ch as u32;
    c < 0x20 || c == 0x7f || (0x80..0xa0).contains(&c)
}

/// Decodes bytes lossily, expands tabs and escapes control characters.
pub fn segments(bytes: &[u8], tab: usize) -> Vec<Seg> {
    let tab = tab.max(1);
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut col = 0usize;
    for ch in bytes.to_str_lossy().chars() {
        if ch == '\t' {
            let n = tab - col % tab;
            cur.extend(std::iter::repeat_n(' ', n));
            col += n;
        } else if is_control(ch) {
            if !cur.is_empty() {
                out.push(Seg {
                    text: std::mem::take(&mut cur),
                    special: false,
                });
            }
            let e = escape(ch);
            col += e.len();
            out.push(Seg {
                text: e,
                special: true,
            });
        } else {
            cur.push(ch);
            col += ch.width().unwrap_or(0);
        }
    }
    if !cur.is_empty() {
        out.push(Seg {
            text: cur,
            special: false,
        });
    }
    out
}

/// Columns `skip..skip+width` of the segments. Wide characters cut by
/// either edge become spaces.
pub fn slice(segs: &[Seg], skip: usize, width: usize) -> Vec<Seg> {
    let mut out: Vec<Seg> = Vec::new();
    let mut col = 0usize;
    let end = skip + width;
    for seg in segs {
        let mut text = String::new();
        for ch in seg.text.chars() {
            let w = ch.width().unwrap_or(0);
            let (start, stop) = (col, col + w);
            col = stop;
            if stop <= skip {
                continue;
            }
            if start >= end {
                break;
            }
            if start < skip || stop > end {
                // Partially visible wide character.
                let visible = stop.min(end) - start.max(skip);
                text.extend(std::iter::repeat_n(' ', visible));
            } else {
                text.push(ch);
            }
        }
        if !text.is_empty() {
            out.push(Seg {
                text,
                special: seg.special,
            });
        }
        if col >= end {
            break;
        }
    }
    out
}

pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Lossy, escaped, single-line display form of a path.
pub fn display(bytes: &[u8]) -> String {
    segments(bytes, 4).into_iter().map(|s| s.text).collect()
}

/// Truncates to `max` columns, keeping the start and adding `…`.
pub fn truncate_end(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_owned();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if w + cw > max - 1 {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}

/// Truncates to `max` columns, keeping the end and adding a leading `…`.
pub fn truncate_start(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_owned();
    }
    if max == 0 {
        return String::new();
    }
    let mut rev: Vec<char> = Vec::new();
    let mut w = 0;
    for ch in s.chars().rev() {
        let cw = ch.width().unwrap_or(0);
        if w + cw > max - 1 {
            break;
        }
        rev.push(ch);
        w += cw;
    }
    let mut out = String::from("…");
    out.extend(rev.into_iter().rev());
    out
}

pub fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| width(&s.content)).sum()
}

/// Cuts spans to `max` columns, ending with `…` if anything was cut.
pub fn truncate_spans(spans: Vec<Span<'static>>, max: usize) -> Vec<Span<'static>> {
    if spans_width(&spans) <= max {
        return spans;
    }
    let mut out = Vec::new();
    let mut used = 0;
    for s in spans {
        let w = width(&s.content);
        if used + w < max {
            used += w;
            out.push(s);
            continue;
        }
        let room = max - used;
        let text = truncate_end(&s.content, room);
        out.push(Span::styled(text, s.style));
        break;
    }
    out
}

/// A line with `left` flush left and `right` flush right in `width`
/// columns. The left side is truncated first.
pub fn left_right(
    left: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    width: usize,
) -> Line<'static> {
    let rw = spans_width(&right);
    if rw >= width {
        return Line::from(truncate_spans(right, width));
    }
    let max_left = if rw == 0 { width } else { width - rw - 1 };
    let left = truncate_spans(left, max_left);
    let lw = spans_width(&left);
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(width.saturating_sub(lw + rw))));
    spans.extend(right);
    Line::from(spans)
}

pub fn styled_segs(segs: Vec<Seg>, base: Style, special: Style) -> Vec<Span<'static>> {
    segs.into_iter()
        .map(|s| Span::styled(s.text, if s.special { special } else { base }))
        .collect()
}

/// `12.4 KB`, `1.0 MB`, `512 B`.
pub fn human_size(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(segs: &[Seg]) -> String {
        segs.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn expands_tabs_and_escapes_controls() {
        let s = segments(b"a\tb\x1b[0m\r", 4);
        assert_eq!(joined(&s), "a   b^[[0m^M");
        assert!(s[1].special);
        assert_eq!(joined(&segments(b"\xff", 4)), "\u{fffd}");
    }

    #[test]
    fn slices_by_columns_with_wide_chars() {
        let s = segments("ab日本c".as_bytes(), 4);
        assert_eq!(joined(&slice(&s, 0, 3)), "ab ");
        assert_eq!(joined(&slice(&s, 3, 4)), " 本c");
        assert_eq!(joined(&slice(&s, 2, 2)), "日");
    }

    #[test]
    fn truncates() {
        assert_eq!(truncate_end("abcdef", 4), "abc…");
        assert_eq!(truncate_start("src/very/long/path.rs", 10), "…g/path.rs");
        assert_eq!(truncate_start("short", 10), "short");
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(12_697), "12.4 KB");
    }
}

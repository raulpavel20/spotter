//! Changed-word detection inside modified lines.
//!
//! Within a hunk, each run of `-` lines followed by a run of `+` lines is a
//! change block. Lines are paired in order, split into tokens, and diffed;
//! the differing tokens become emphasis ranges on both lines.

use similar::{Algorithm, DiffTag, capture_diff_slices};

use crate::model::{FilePatch, Line, LineKind};

/// Longer lines are left unannotated.
pub const MAX_LINE: usize = 1000;
/// Pairs sharing less than this fraction of non-whitespace bytes are
/// treated as rewrites and get no word emphasis.
pub const MIN_SHARED: f64 = 0.4;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Word,
    Space,
    Other,
}

fn class(c: char) -> Class {
    if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else if c.is_whitespace() {
        Class::Space
    } else {
        Class::Other
    }
}

/// Byte ranges of tokens: word runs, whitespace runs, single punctuation.
fn tokens(s: &str) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut prev: Option<Class> = None;
    for (i, c) in s.char_indices() {
        let k = class(c);
        let end = i + c.len_utf8();
        match (prev, out.last_mut()) {
            (Some(p), Some(last)) if p == k && k != Class::Other => last.1 = end,
            _ => out.push((i, end)),
        }
        prev = Some(k);
    }
    out
}

fn non_ws(s: &str) -> usize {
    s.bytes().filter(|b| !b.is_ascii_whitespace()).count()
}

type Spans = Vec<(u32, u32)>;

/// Emphasis ranges for a deleted/added pair, or `None` if the lines are
/// too different (or too long) to be worth annotating.
pub fn pair(old: &[u8], new: &[u8]) -> Option<(Spans, Spans)> {
    if old.len() > MAX_LINE || new.len() > MAX_LINE {
        return None;
    }
    let (old, new) = (
        std::str::from_utf8(old).ok()?,
        std::str::from_utf8(new).ok()?,
    );
    let (ot, nt) = (tokens(old), tokens(new));
    let ow: Vec<&str> = ot.iter().map(|&(a, b)| &old[a..b]).collect();
    let nw: Vec<&str> = nt.iter().map(|&(a, b)| &new[a..b]).collect();
    let mut shared = 0;
    let (mut os, mut ns) = (Vec::new(), Vec::new());
    for op in capture_diff_slices(Algorithm::Myers, &ow, &nw) {
        let (tag, or, nr) = op.as_tag_tuple();
        if tag == DiffTag::Equal {
            shared += ow[or].iter().map(|t| non_ws(t)).sum::<usize>();
            continue;
        }
        if !or.is_empty() {
            os.push((ot[or.start].0, ot[or.end - 1].1));
        }
        if !nr.is_empty() {
            ns.push((nt[nr.start].0, nt[nr.end - 1].1));
        }
    }
    let total = non_ws(old).max(non_ws(new));
    if total == 0 || (shared as f64) < MIN_SHARED * total as f64 {
        return None;
    }
    Some((merge(old, os), merge(new, ns)))
}

/// Joins spans separated only by whitespace, so `foo bar` → `baz qux`
/// reads as one change rather than two.
fn merge(s: &str, spans: Vec<(usize, usize)>) -> Spans {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (a, b) in spans {
        match out.last_mut() {
            Some(last) if s[last.1..a].chars().all(char::is_whitespace) => last.1 = b,
            _ => out.push((a, b)),
        }
    }
    out.into_iter().map(|(a, b)| (a as u32, b as u32)).collect()
}

/// Fills `Line::emph` for every hunk of the patch.
pub fn annotate(p: &mut FilePatch) {
    for hunk in &mut p.hunks {
        annotate_lines(&mut hunk.lines);
    }
}

fn annotate_lines(lines: &mut [Line]) {
    let n = lines.len();
    let mut i = 0;
    while i < n {
        if lines[i].kind != LineKind::Del {
            i += 1;
            continue;
        }
        // "\ No newline" markers may sit between lines of a block.
        let mut dels = Vec::new();
        while i < n && matches!(lines[i].kind, LineKind::Del | LineKind::NoNewline) {
            if lines[i].kind == LineKind::Del {
                dels.push(i);
            }
            i += 1;
        }
        let mut adds = Vec::new();
        while i < n && matches!(lines[i].kind, LineKind::Add | LineKind::NoNewline) {
            if lines[i].kind == LineKind::Add {
                adds.push(i);
            }
            i += 1;
        }
        for (&d, &a) in dels.iter().zip(&adds) {
            if let Some((o, nw)) = pair(&lines[d].text, &lines[a].text) {
                lines[d].emph = o;
                lines[a].emph = nw;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::patch::parse_patch;

    fn show(s: &str, spans: &[(u32, u32)]) -> Vec<String> {
        spans
            .iter()
            .map(|&(a, b)| s[a as usize..b as usize].to_owned())
            .collect()
    }

    #[test]
    fn marks_changed_words() {
        let old = "\tlabel.text = item.name";
        let new = "\tlabel.text = item.display_name";
        let (o, n) = pair(old.as_bytes(), new.as_bytes()).unwrap();
        assert_eq!(show(old, &o), ["name"]);
        assert_eq!(show(new, &n), ["display_name"]);
    }

    #[test]
    fn joins_neighbouring_changes_and_handles_insertions() {
        let old = "let x = foo(a, b);";
        let new = "let x = bar(a, b, c);";
        let (o, n) = pair(old.as_bytes(), new.as_bytes()).unwrap();
        assert_eq!(show(old, &o), ["foo"]);
        assert_eq!(show(new, &n), ["bar", ", c"]);
        let old = "if a and b:";
        let new = "if c or d:";
        // Too different: no emphasis.
        assert!(pair(old.as_bytes(), new.as_bytes()).is_none());
        let old = "return one two three four five";
        let new = "return uno dos three four five";
        let (o, _) = pair(old.as_bytes(), new.as_bytes()).unwrap();
        assert_eq!(show(old, &o), ["one two"]);
    }

    #[test]
    fn unicode_and_limits() {
        let old = "名前 = \"ä\"";
        let new = "名前 = \"ö\"";
        let (o, n) = pair(old.as_bytes(), new.as_bytes()).unwrap();
        assert_eq!(show(old, &o), ["ä"]);
        assert_eq!(show(new, &n), ["ö"]);
        let long = "x".repeat(MAX_LINE + 1);
        assert!(pair(long.as_bytes(), b"x").is_none());
        assert!(pair(b"\xff abc def", b"\xfe abc def").is_none());
    }

    #[test]
    fn annotates_blocks_in_order() {
        let patch = "diff --git a/a b/a
--- a/a
+++ b/a
@@ -1,4 +1,4 @@
 keep
-let total = price * qty;
-print(total)
+let total = price * quantity;
+print(total, currency)
 end
";
        let mut p = parse_patch(patch.as_bytes()).remove(0);
        annotate(&mut p);
        let l = &p.hunks[0].lines;
        assert!(l[0].emph.is_empty());
        let t = |i: usize| l[i].text.to_string();
        assert_eq!(show(&t(1), &l[1].emph), ["qty"]);
        assert_eq!(show(&t(3), &l[3].emph), ["quantity"]);
        assert_eq!(show(&t(4), &l[4].emph), [", currency"]);
        assert!(l[5].emph.is_empty());
    }

    #[test]
    fn pure_additions_have_no_emphasis() {
        let patch = "diff --git a/a b/a
--- a/a
+++ b/a
@@ -1 +1,2 @@
 keep
+new
";
        let mut p = parse_patch(patch.as_bytes()).remove(0);
        annotate(&mut p);
        assert!(p.hunks[0].lines.iter().all(|l| l.emph.is_empty()));
    }
}

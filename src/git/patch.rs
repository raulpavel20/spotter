//! Unified diff parsing, and pairing parsed patches with file lists.

use std::collections::HashMap;

use bstr::{BString, ByteSlice};

use super::cmd::{Git, GitError};
use super::diff::DiffSpec;
use crate::model::{FileChange, FilePatch, Hunk, Line, LineKind};

/// Quotes a path the way git does in `diff --git` headers with
/// `core.quotepath=off`: control characters, `"` and `\` force quoting,
/// high bytes are left alone.
pub fn quote_path(prefix: &str, path: &[u8]) -> BString {
    let needs = path
        .iter()
        .any(|&b| b < 0x20 || b == b'"' || b == b'\\' || b == 0x7f);
    let mut out = BString::from(Vec::with_capacity(path.len() + prefix.len() + 2));
    if !needs {
        out.extend_from_slice(prefix.as_bytes());
        out.extend_from_slice(path);
        return out;
    }
    out.push(b'"');
    out.extend_from_slice(prefix.as_bytes());
    for &b in path {
        match b {
            0x07 => out.extend_from_slice(b"\\a"),
            0x08 => out.extend_from_slice(b"\\b"),
            b'\t' => out.extend_from_slice(b"\\t"),
            b'\n' => out.extend_from_slice(b"\\n"),
            0x0b => out.extend_from_slice(b"\\v"),
            0x0c => out.extend_from_slice(b"\\f"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b if b < 0x20 || b == 0x7f => {
                out.extend_from_slice(format!("\\{b:03o}").as_bytes());
            }
            b => out.push(b),
        }
    }
    out.push(b'"');
    out
}

/// The `diff --git` line git prints for a file change.
pub fn expected_header(f: &FileChange) -> BString {
    let old = f.old_path.as_ref().unwrap_or(&f.path);
    let mut h = BString::from("diff --git ");
    h.extend_from_slice(&quote_path("a/", old));
    h.push(b' ');
    h.extend_from_slice(&quote_path("b/", &f.path));
    h
}

fn parse_range(s: &[u8]) -> (u32, u32) {
    let s = s.to_str_lossy();
    let mut it = s.splitn(2, ',');
    let start = it.next().and_then(|n| n.parse().ok()).unwrap_or(0);
    let len = it.next().map_or(1, |n| n.parse().unwrap_or(0));
    (start, len)
}

/// Parses `@@ -a,b +c,d @@ section`.
fn parse_hunk_header(line: &[u8]) -> Option<Hunk> {
    let rest = line.strip_prefix(b"@@ -")?;
    let mut parts = rest.splitn_str(3, " ");
    let old = parts.next()?;
    let new = parts.next()?.strip_prefix(b"+")?;
    let (old_start, old_len) = parse_range(old);
    let (new_start, new_len) = parse_range(new);
    Some(Hunk {
        old_start,
        old_len,
        new_start,
        new_len,
        header: BString::from(line),
        lines: Vec::new(),
    })
}

const SKIP_META: &[&[u8]] = &[b"index ", b"--- ", b"+++ "];

/// State of the hunk being read: remaining old/new lines and the next
/// old/new line numbers.
struct Open {
    old_rem: u32,
    new_rem: u32,
    old_no: u32,
    new_no: u32,
}

/// Parses `git diff -p` output into one [`FilePatch`] per `diff --git`.
pub fn parse_patch(out: &[u8]) -> Vec<FilePatch> {
    let mut files: Vec<FilePatch> = Vec::new();
    let mut open: Option<Open> = None;
    let body = out.strip_suffix(b"\n").unwrap_or(out);
    if body.is_empty() {
        return files;
    }
    for line in body.split_str("\n") {
        if let (Some(st), Some(file)) = (&mut open, files.last_mut()) {
            let has_rem = st.old_rem > 0 || st.new_rem > 0;
            let kind = match line.first() {
                Some(b'\\') => Some(LineKind::NoNewline),
                Some(b'+') if st.new_rem > 0 => Some(LineKind::Add),
                Some(b'-') if st.old_rem > 0 => Some(LineKind::Del),
                Some(b' ') | None if has_rem => Some(LineKind::Context),
                _ => None,
            };
            if let Some(kind) = kind {
                let text = match kind {
                    LineKind::NoNewline => line,
                    _ => line.get(1..).unwrap_or_default(),
                };
                let (old, new) = match kind {
                    LineKind::Context => (Some(st.old_no), Some(st.new_no)),
                    LineKind::Del => (Some(st.old_no), None),
                    LineKind::Add => (None, Some(st.new_no)),
                    LineKind::NoNewline => (None, None),
                };
                if old.is_some() {
                    st.old_no += 1;
                    st.old_rem = st.old_rem.saturating_sub(1);
                }
                if new.is_some() {
                    st.new_no += 1;
                    st.new_rem = st.new_rem.saturating_sub(1);
                }
                let hunk = file.hunks.last_mut().expect("open hunk");
                hunk.lines.push(Line {
                    kind,
                    old,
                    new,
                    text: BString::from(text),
                    emph: Vec::new(),
                });
                continue;
            }
            open = None;
        }
        if line.starts_with(b"diff --git ") || line.starts_with(b"diff --cc ") {
            files.push(FilePatch {
                header: BString::from(line),
                ..FilePatch::default()
            });
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        if line.starts_with(b"@@ ") {
            if let Some(h) = parse_hunk_header(line) {
                open = Some(Open {
                    old_rem: h.old_len,
                    new_rem: h.new_len,
                    old_no: h.old_start,
                    new_no: h.new_start,
                });
                file.hunks.push(h);
            }
            continue;
        }
        if line.starts_with(b"Binary files ") || line.starts_with(b"GIT binary patch") {
            file.binary = true;
            continue;
        }
        if line.is_empty() || SKIP_META.iter().any(|p| line.starts_with(p)) {
            continue;
        }
        file.meta.push(BString::from(line));
    }
    files
}

/// Pairs patches with files by their `diff --git` header. Files without a
/// matching section get `None` (e.g. the worktree changed between calls).
pub fn attach(files: &[FileChange], patches: Vec<FilePatch>) -> Vec<Option<FilePatch>> {
    let mut index: HashMap<BString, usize> = HashMap::new();
    for (i, f) in files.iter().enumerate() {
        index.entry(expected_header(f)).or_insert(i);
    }
    let mut out: Vec<Option<FilePatch>> = vec![None; files.len()];
    for p in patches {
        let Some(&i) = index.get(&p.header) else {
            continue;
        };
        match &mut out[i] {
            // A type change prints two sections with the same header.
            Some(existing) => {
                existing.meta.extend(p.meta);
                existing.hunks.extend(p.hunks);
                existing.binary |= p.binary;
            }
            slot => *slot = Some(p),
        }
    }
    out
}

/// Loads the patch for `spec`, optionally limited to some paths.
pub fn load(
    git: &Git,
    spec: &DiffSpec,
    paths: Option<&[&[u8]]>,
) -> Result<Vec<FilePatch>, GitError> {
    let mut cmd = spec.base_cmd(git).arg("-p");
    if let Some(paths) = paths {
        cmd = cmd.arg("--");
        for p in paths {
            cmd = cmd.arg(bytes_to_os(p));
        }
    }
    Ok(parse_patch(&cmd.out()?))
}

#[cfg(unix)]
pub fn bytes_to_os(b: &[u8]) -> std::ffi::OsString {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::OsStr::from_bytes(b).to_owned()
}

#[cfg(not(unix))]
pub fn bytes_to_os(b: &[u8]) -> std::ffi::OsString {
    b.to_str_lossy().into_owned().into()
}

/// Builds the patch of an untracked text file: every line is an addition.
pub fn untracked_patch(f: &FileChange, content: &[u8]) -> FilePatch {
    let mut lines = Vec::new();
    let body = content.strip_suffix(b"\n");
    let missing_newline = !content.is_empty() && body.is_none();
    let body = body.unwrap_or(content);
    if !content.is_empty() {
        for (i, l) in body.split_str("\n").enumerate() {
            lines.push(Line {
                kind: LineKind::Add,
                old: None,
                new: Some(i as u32 + 1),
                text: BString::from(l),
                emph: Vec::new(),
            });
        }
    }
    let n = lines.len() as u32;
    if missing_newline {
        lines.push(Line {
            kind: LineKind::NoNewline,
            old: None,
            new: None,
            text: BString::from("\\ No newline at end of file"),
            emph: Vec::new(),
        });
    }
    let hunks = if n == 0 {
        Vec::new()
    } else {
        vec![Hunk {
            old_start: 0,
            old_len: 0,
            new_start: 1,
            new_len: n,
            header: BString::from(format!("@@ -0,0 +1,{n} @@")),
            lines,
        }]
    };
    FilePatch {
        header: expected_header(f),
        meta: vec![BString::from(format!("new file mode {:o}", f.new_mode))],
        binary: false,
        hunks,
        ..FilePatch::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Status;

    const SAMPLE: &str = "diff --git a/src/tooltip.gd b/src/tooltip.gd
index 1111111..2222222 100644
--- a/src/tooltip.gd
+++ b/src/tooltip.gd
@@ -10,7 +10,8 @@ func _ready():
 \tvar label = Label.new()
-\tlabel.text = item.name
+\tlabel.text = item.display_name
+\tlabel.tooltip_text = item.description
 \tadd_child(label)



@@ -40 +41 @@ func x():
-a
\\ No newline at end of file
+b
\\ No newline at end of file
diff --git a/img.png b/img.png
index 3333333..4444444 100644
Binary files a/img.png and b/img.png differ
diff --git a/run.sh b/run.sh
old mode 100644
new mode 100755
diff --git a/old name.txt b/new name.txt
similarity index 92%
rename from old name.txt
rename to new name.txt
index 5555555..6666666 100644
--- a/old name.txt
+++ b/new name.txt
@@ -1,2 +1,2 @@
 keep
--- not a header
+++ not a header either
";

    #[test]
    fn parses_hunks_numbers_and_markers() {
        let files = parse_patch(SAMPLE.as_bytes());
        assert_eq!(files.len(), 4);
        let f = &files[0];
        assert_eq!(f.hunks.len(), 2);
        let h = &f.hunks[0];
        assert_eq!(
            (h.old_start, h.old_len, h.new_start, h.new_len),
            (10, 7, 10, 8)
        );
        assert_eq!(h.lines.len(), 8);
        assert_eq!(h.lines[1].kind, LineKind::Del);
        assert_eq!(h.lines[1].old, Some(11));
        assert_eq!(h.lines[2].new, Some(11));
        assert_eq!(h.lines[3].new, Some(12));
        assert_eq!((h.lines[4].old, h.lines[4].new), (Some(12), Some(13)));
        let h2 = &f.hunks[1];
        assert_eq!(
            (h2.old_start, h2.old_len, h2.new_start, h2.new_len),
            (40, 1, 41, 1)
        );
        let kinds: Vec<_> = h2.lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            [
                LineKind::Del,
                LineKind::NoNewline,
                LineKind::Add,
                LineKind::NoNewline
            ]
        );
        assert!(files[1].binary);
        assert_eq!(files[2].meta, ["old mode 100644", "new mode 100755"]);
        assert!(files[3].meta.iter().any(|m| m == "rename to new name.txt"));
        let lines = &files[3].hunks[0].lines;
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1].text, "-- not a header");
        assert_eq!(lines[2].text, "++ not a header either");
    }

    fn change(path: &str, old: Option<&str>) -> FileChange {
        FileChange {
            path: path.into(),
            old_path: old.map(Into::into),
            status: if old.is_some() {
                Status::Renamed(92)
            } else {
                Status::Modified
            },
            old_mode: 0o100644,
            new_mode: 0o100644,
            old_oid: String::new(),
            new_oid: String::new(),
            added: Some(1),
            deleted: Some(1),
            collapse: None,
            size: None,
        }
    }

    #[test]
    fn quotes_like_git() {
        assert_eq!(quote_path("a/", b"plain name.txt"), "a/plain name.txt");
        assert_eq!(quote_path("a/", b"nl\nname"), "\"a/nl\\nname\"");
        assert_eq!(quote_path("b/", "ü.txt".as_bytes()), "b/ü.txt");
        assert_eq!(quote_path("b/", b"q\"\\\x01"), "\"b/q\\\"\\\\\\001\"");
    }

    #[test]
    fn attaches_by_header_regardless_of_order() {
        let files = vec![
            change("new name.txt", Some("old name.txt")),
            change("src/tooltip.gd", None),
            change("missing.txt", None),
        ];
        let attached = attach(&files, parse_patch(SAMPLE.as_bytes()));
        assert_eq!(attached[0].as_ref().unwrap().hunks.len(), 1);
        assert_eq!(attached[1].as_ref().unwrap().hunks.len(), 2);
        assert!(attached[2].is_none());
    }

    #[test]
    fn synthesizes_untracked_patch() {
        let f = change("new.txt", None);
        let p = untracked_patch(&f, b"one\ntwo");
        assert_eq!(p.hunks[0].new_len, 2);
        assert_eq!(p.hunks[0].lines.len(), 3);
        assert_eq!(p.hunks[0].lines[2].kind, LineKind::NoNewline);
        let p = untracked_patch(&f, b"");
        assert!(p.hunks.is_empty());
    }
}

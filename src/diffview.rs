//! The diff view's state: a whole target flattened into display rows, with
//! a cursor, scroll offsets and navigation (PLAN §4 "Diff view").

use std::collections::HashSet;

use bstr::{BString, ByteSlice};

use crate::git::diff::DiffSpec;
use crate::model::{FileChange, FilePatch, LineKind, Status, TargetId};
use crate::refresh::MAX_UNTRACKED;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    FileHeader,
    /// Index into the patch's `meta`.
    Meta(usize),
    Hunk(usize),
    /// Hunk index, line index.
    Line(usize, usize),
    Collapsed,
    Binary,
    Loading,
    NoChanges,
    /// An untracked file too big to show.
    TooLarge,
    /// Blank separator between files.
    Gap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub file: usize,
    pub kind: RowKind,
}

/// Extended header lines that the file header already conveys.
const REDUNDANT_META: &[&str] = &[
    "similarity index",
    "dissimilarity index",
    "rename from",
    "rename to",
    "copy from",
    "copy to",
    "new file mode",
    "deleted file mode",
    "index ",
];

pub fn shown_meta(meta: &[u8]) -> bool {
    !REDUNDANT_META
        .iter()
        .any(|p| meta.starts_with(p.as_bytes()))
}

#[derive(Debug, Clone)]
pub struct DiffView {
    pub target: TargetId,
    pub spec: DiffSpec,
    pub files: Vec<FileChange>,
    /// `None`: not loaded yet.
    pub patches: Vec<Option<FilePatch>>,
    pub loading: bool,
    pub lazy: bool,
    /// Collapsed files the user expanded, by path (survives reloads).
    pub expanded: HashSet<BString>,
    /// Files whose lazy load is in flight.
    pub requested: HashSet<usize>,
    pub rows: Vec<Row>,
    pub file_rows: Vec<usize>,
    pub cursor: usize,
    pub scroll: usize,
    pub hscroll: usize,
    pub viewport: usize,
    /// Width of each line-number column.
    pub gutter: usize,
    pub error: Option<String>,
}

impl DiffView {
    pub fn new(target: TargetId, spec: DiffSpec, files: Vec<FileChange>) -> Self {
        let n = files.len();
        let mut v = DiffView {
            target,
            spec,
            files,
            patches: vec![None; n],
            loading: true,
            lazy: false,
            expanded: HashSet::new(),
            requested: HashSet::new(),
            rows: Vec::new(),
            file_rows: Vec::new(),
            cursor: 0,
            scroll: 0,
            hscroll: 0,
            viewport: 20,
            gutter: 4,
            error: None,
        };
        v.rebuild();
        v
    }

    pub fn is_collapsed(&self, i: usize) -> bool {
        self.files[i].collapse.is_some() && !self.expanded.contains(&self.files[i].path)
    }

    /// Recomputes rows from files and patches.
    pub fn rebuild(&mut self) {
        let mut rows = Vec::new();
        let mut file_rows = Vec::with_capacity(self.files.len());
        let mut max_no = 0u32;
        for (i, _) in self.files.iter().enumerate() {
            if i > 0 {
                rows.push(Row {
                    file: i,
                    kind: RowKind::Gap,
                });
            }
            file_rows.push(rows.len());
            rows.push(Row {
                file: i,
                kind: RowKind::FileHeader,
            });
            if self.is_collapsed(i) {
                rows.push(Row {
                    file: i,
                    kind: RowKind::Collapsed,
                });
                continue;
            }
            let f = &self.files[i];
            if f.status == Status::Untracked
                && f.size.unwrap_or(0) > MAX_UNTRACKED
                && !f.is_binary()
            {
                rows.push(Row {
                    file: i,
                    kind: RowKind::TooLarge,
                });
                continue;
            }
            let Some(p) = self.patches.get(i).and_then(Option::as_ref) else {
                rows.push(Row {
                    file: i,
                    kind: RowKind::Loading,
                });
                continue;
            };
            let mut shown = 0;
            for (m, line) in p.meta.iter().enumerate() {
                if shown_meta(line) {
                    rows.push(Row {
                        file: i,
                        kind: RowKind::Meta(m),
                    });
                    shown += 1;
                }
            }
            if p.binary {
                rows.push(Row {
                    file: i,
                    kind: RowKind::Binary,
                });
                continue;
            }
            if p.hunks.is_empty() && shown == 0 {
                rows.push(Row {
                    file: i,
                    kind: RowKind::NoChanges,
                });
            }
            for (h, hunk) in p.hunks.iter().enumerate() {
                rows.push(Row {
                    file: i,
                    kind: RowKind::Hunk(h),
                });
                max_no = max_no
                    .max(hunk.old_start + hunk.old_len)
                    .max(hunk.new_start + hunk.new_len);
                for l in 0..hunk.lines.len() {
                    rows.push(Row {
                        file: i,
                        kind: RowKind::Line(h, l),
                    });
                }
            }
        }
        self.gutter = max_no.to_string().len().max(3);
        self.rows = rows;
        self.file_rows = file_rows;
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
        self.clamp_scroll();
    }

    /// Toggles the collapsed state of the file under the cursor.
    pub fn toggle_expand(&mut self) -> bool {
        let Some(i) = self.current_file() else {
            return false;
        };
        if self.files[i].collapse.is_none() {
            return false;
        }
        let path = self.files[i].path.clone();
        if !self.expanded.remove(&path) {
            self.expanded.insert(path);
        }
        self.rebuild();
        self.jump_to_file(i);
        true
    }

    /// Replaces files and patches together (a reload), keeping the cursor
    /// on the same file and offset when the file still exists.
    pub fn replace(
        &mut self,
        spec: DiffSpec,
        files: Vec<FileChange>,
        patches: Vec<Option<FilePatch>>,
        lazy: bool,
    ) {
        let anchor = self.anchor();
        self.spec = spec;
        self.files = files;
        self.set_patches(patches, lazy);
        self.restore(anchor);
    }

    pub fn set_patches(&mut self, patches: Vec<Option<FilePatch>>, lazy: bool) {
        self.patches = patches;
        self.lazy = lazy;
        self.loading = false;
        self.requested.clear();
        self.rebuild();
    }

    pub fn set_file_patch(&mut self, i: usize, p: FilePatch) {
        if i < self.patches.len() {
            // Keep the cursor on the same row identity across the rebuild.
            let anchor = self.anchor();
            self.patches[i] = Some(p);
            self.requested.remove(&i);
            self.rebuild();
            self.restore(anchor);
        }
    }

    // --- position -------------------------------------------------------

    pub fn current_file(&self) -> Option<usize> {
        self.rows.get(self.cursor).map(|r| r.file)
    }

    /// Where the cursor is, in terms that survive a reload: file path,
    /// offset within the file, and the cursor's screen position.
    pub fn anchor(&self) -> Option<(Vec<u8>, usize, usize)> {
        let file = self.current_file()?;
        let off = self.cursor - self.file_rows[file];
        Some((
            self.files[file].path.to_vec(),
            off,
            self.cursor.saturating_sub(self.scroll),
        ))
    }

    pub fn restore(&mut self, anchor: Option<(Vec<u8>, usize, usize)>) {
        let Some((path, off, screen)) = anchor else {
            return;
        };
        let Some(i) = self.files.iter().position(|f| f.path.as_bytes() == path) else {
            return;
        };
        let start = self.file_rows[i];
        let end = self
            .file_rows
            .get(i + 1)
            .copied()
            .unwrap_or(self.rows.len());
        self.cursor = (start + off).min(end.saturating_sub(1)).max(start);
        self.scroll = self.cursor.saturating_sub(screen);
        self.clamp_scroll();
    }

    pub fn clamp_scroll(&mut self) {
        let h = self.viewport.max(1);
        let max_scroll = self.rows.len().saturating_sub(h);
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + h {
            self.scroll = self.cursor + 1 - h;
        }
        self.scroll = self.scroll.min(max_scroll);
    }

    pub fn move_by(&mut self, delta: isize) {
        let last = self.rows.len().saturating_sub(1) as isize;
        self.cursor = (self.cursor as isize + delta).clamp(0, last.max(0)) as usize;
        self.clamp_scroll();
    }

    pub fn page(&mut self, delta: isize) {
        let before = self.cursor;
        self.move_by(delta);
        let moved = self.cursor as isize - before as isize;
        let max_scroll = self.rows.len().saturating_sub(self.viewport.max(1)) as isize;
        self.scroll = (self.scroll as isize + moved).clamp(0, max_scroll.max(0)) as usize;
        self.clamp_scroll();
    }

    pub fn top(&mut self) {
        self.cursor = 0;
        self.clamp_scroll();
    }

    pub fn bottom(&mut self) {
        self.cursor = self.rows.len().saturating_sub(1);
        self.clamp_scroll();
    }

    /// Moves the cursor to `row` and scrolls it to the top of the view.
    pub fn jump(&mut self, row: usize) {
        self.cursor = row.min(self.rows.len().saturating_sub(1));
        let max_scroll = self.rows.len().saturating_sub(self.viewport.max(1));
        self.scroll = self.cursor.min(max_scroll);
        self.clamp_scroll();
    }

    pub fn jump_to_file(&mut self, i: usize) {
        if let Some(&row) = self.file_rows.get(i) {
            self.jump(row);
        }
    }

    pub fn next_hunk(&mut self) -> bool {
        let found = (self.cursor + 1..self.rows.len())
            .find(|&r| matches!(self.rows[r].kind, RowKind::Hunk(_)));
        found.map(|r| self.jump(r)).is_some()
    }

    pub fn prev_hunk(&mut self) -> bool {
        let found = (0..self.cursor)
            .rev()
            .find(|&r| matches!(self.rows[r].kind, RowKind::Hunk(_)));
        found.map(|r| self.jump(r)).is_some()
    }

    pub fn next_file(&mut self) -> bool {
        let found = self.file_rows.iter().copied().find(|&r| r > self.cursor);
        found.map(|r| self.jump(r)).is_some()
    }

    pub fn prev_file(&mut self) -> bool {
        let cur = self.current_file().unwrap_or(0);
        let start = self.file_rows.get(cur).copied().unwrap_or(0);
        // From inside a file, go to its header first.
        let target = if self.cursor > start {
            Some(start)
        } else {
            self.file_rows
                .iter()
                .copied()
                .rev()
                .find(|&r| r < self.cursor)
        };
        target.map(|r| self.jump(r)).is_some()
    }

    pub fn scroll_h(&mut self, delta: isize) {
        self.hscroll = (self.hscroll as isize + delta).max(0) as usize;
    }

    /// Files touching the viewport that still need loading.
    pub fn visible_unloaded(&self) -> Vec<usize> {
        let end = (self.scroll + self.viewport.max(1)).min(self.rows.len());
        let mut out = Vec::new();
        for r in &self.rows[self.scroll.min(end)..end] {
            if r.kind == RowKind::Loading
                && !self.requested.contains(&r.file)
                && !out.contains(&r.file)
            {
                out.push(r.file);
            }
        }
        out
    }

    /// New-side line number under the cursor, for opening an editor.
    pub fn cursor_line(&self) -> Option<u32> {
        let row = self.rows.get(self.cursor)?;
        let p = self.patches.get(row.file)?.as_ref();
        match row.kind {
            RowKind::Line(h, l) => {
                let hunk = &p?.hunks[h];
                let line = &hunk.lines[l];
                if let Some(n) = line.new {
                    return Some(n);
                }
                // A deleted line: the next new-side line, else the hunk end.
                hunk.lines[l..]
                    .iter()
                    .find_map(|x| x.new)
                    .or(Some(hunk.new_start + hunk.new_len.saturating_sub(1)))
                    .map(|n| n.max(1))
            }
            RowKind::Hunk(h) => Some(p?.hunks[h].new_start.max(1)),
            _ => first_changed_line(p?),
        }
    }
}

/// First added (or, failing that, changed) line of a patch.
pub fn first_changed_line(p: &FilePatch) -> Option<u32> {
    for h in &p.hunks {
        for (i, l) in h.lines.iter().enumerate() {
            match l.kind {
                LineKind::Add => return l.new,
                LineKind::Del => {
                    return h.lines[i..]
                        .iter()
                        .find_map(|x| x.new)
                        .or(Some(h.new_start.max(1)));
                }
                _ => {}
            }
        }
    }
    p.hunks.first().map(|h| h.new_start.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::patch::parse_patch;

    fn file(path: &str) -> FileChange {
        FileChange {
            path: path.into(),
            old_path: None,
            status: Status::Modified,
            old_mode: 0o100644,
            new_mode: 0o100644,
            old_oid: "1".into(),
            new_oid: "2".into(),
            added: Some(1),
            deleted: Some(1),
            collapse: None,
            size: None,
        }
    }

    fn view() -> DiffView {
        let patch = "diff --git a/a b/a
--- a/a
+++ b/a
@@ -1,3 +1,3 @@
 x
-y
+Y
 z
@@ -10,2 +10,3 @@
 p
+q
 r
diff --git a/b b/b
--- a/b
+++ b/b
@@ -5 +5 @@
-old
+new
";
        let parsed = parse_patch(patch.as_bytes());
        let mut v = DiffView::new(
            TargetId::Uncommitted,
            DiffSpec::Worktree("HEAD".into()),
            vec![file("a"), file("b")],
        );
        v.viewport = 5;
        v.set_patches(parsed.into_iter().map(Some).collect(), false);
        v
    }

    #[test]
    fn builds_rows_and_navigates() {
        let mut v = view();
        // header, hunk, 4 lines, hunk, 3 lines, gap, header, hunk, 2 lines
        assert_eq!(v.rows.len(), 1 + 1 + 4 + 1 + 3 + 1 + 1 + 1 + 2);
        assert_eq!(v.file_rows, [0, 11]);
        assert!(v.next_hunk());
        assert_eq!(v.rows[v.cursor].kind, RowKind::Hunk(0));
        assert!(v.next_hunk());
        assert_eq!(v.rows[v.cursor].kind, RowKind::Hunk(1));
        assert!(v.next_file());
        assert_eq!(v.current_file(), Some(1));
        assert!(v.prev_file());
        assert_eq!(v.cursor, 0);
        v.bottom();
        assert!(v.scroll + v.viewport >= v.rows.len());
    }

    #[test]
    fn cursor_line_maps_to_new_side() {
        let mut v = view();
        v.cursor = 3; // "-y"
        assert_eq!(v.cursor_line(), Some(2));
        v.cursor = 4; // "+Y"
        assert_eq!(v.cursor_line(), Some(2));
        v.cursor = 0; // header: first change
        assert_eq!(v.cursor_line(), Some(2));
    }

    #[test]
    fn anchor_survives_rebuild() {
        let mut v = view();
        v.jump_to_file(1);
        v.move_by(2);
        let a = v.anchor();
        let cursor = v.cursor;
        v.rebuild();
        v.restore(a);
        assert_eq!(v.cursor, cursor);
    }

    #[test]
    fn collapsed_files_have_one_row() {
        let mut v = view();
        v.files[0].collapse = Some(crate::model::Collapse::Lockfile);
        v.rebuild();
        assert_eq!(v.rows[1].kind, RowKind::Collapsed);
        v.expanded.insert("a".into());
        v.rebuild();
        assert_eq!(v.rows[1].kind, RowKind::Hunk(0));
    }
}

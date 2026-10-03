//! The diff view's state: a whole target flattened into display rows, a
//! pager-style scroll position, folding, and navigation.
//!
//! There is no line cursor: the *current file* is the file of the row at
//! the top of the viewport. Once a file's own header has scrolled off, the
//! UI pins a copy of it to the top line (see [`DiffView::sticky`]).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bstr::{BString, ByteSlice};

use crate::git::diff::{DiffOpts, DiffSpec};
use crate::highlight::FileHighlight;
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
    /// Why an unviewed file starts collapsed (lockfile, generated…).
    Collapsed,
    Binary,
    Loading,
    NoChanges,
    /// An untracked file too big to show.
    TooLarge,
    /// Rule between files.
    Separator,
    /// Blank line above every hunk but a file's first.
    Blank,
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
    /// Per-file fold choices by path, overriding the default (see
    /// [`DiffView::is_folded`]); they survive reloads.
    pub fold: HashMap<BString, bool>,
    /// Whether each file is marked viewed (kept in sync by the app).
    pub viewed: Vec<bool>,
    /// Files whose lazy load is in flight.
    pub requested: HashSet<usize>,
    pub rows: Vec<Row>,
    pub file_rows: Vec<usize>,
    /// The first row on screen.
    pub scroll: usize,
    pub hscroll: usize,
    pub viewport: usize,
    /// Width of each line-number column.
    pub gutter: usize,
    pub error: Option<String>,
    /// Content key of each file (`FileChange::mark_key`).
    pub keys: Vec<String>,
    /// Highlight cache keys: content key plus diff options.
    pub hl_keys: Vec<String>,
    /// How patches were produced (context lines, whitespace).
    pub opts: DiffOpts,
    /// Viewed files start folded (`review.collapse_viewed`).
    pub collapse_viewed: bool,
    /// Syntax colors by content key; they survive reloads of unchanged
    /// files.
    pub highlights: HashMap<String, Arc<FileHighlight>>,
    /// Content keys whose highlighting is in flight.
    pub hl_requested: HashSet<String>,
    pub explorer_offset: usize,
}

/// A position that survives a rebuild: file path and row offset in it.
pub type Anchor = Option<(Vec<u8>, usize)>;

impl DiffView {
    pub fn new(target: TargetId, spec: DiffSpec, files: Vec<FileChange>, opts: DiffOpts) -> Self {
        let n = files.len();
        let mut v = DiffView {
            target,
            spec,
            files,
            patches: vec![None; n],
            loading: true,
            lazy: false,
            fold: HashMap::new(),
            viewed: vec![false; n],
            requested: HashSet::new(),
            rows: Vec::new(),
            file_rows: Vec::new(),
            scroll: 0,
            hscroll: 0,
            viewport: 20,
            gutter: 4,
            error: None,
            keys: Vec::new(),
            hl_keys: Vec::new(),
            opts,
            collapse_viewed: true,
            highlights: HashMap::new(),
            hl_requested: HashSet::new(),
            explorer_offset: 0,
        };
        v.set_keys();
        v.rebuild();
        v
    }

    fn set_keys(&mut self) {
        self.keys = self.files.iter().map(FileChange::mark_key).collect();
        let sfx = self.opts.suffix();
        self.hl_keys = self.keys.iter().map(|k| format!("{k}{sfx}")).collect();
    }

    /// New diff options; the caller reloads the patch.
    pub fn set_opts(&mut self, opts: DiffOpts) {
        self.opts = opts;
        self.set_keys();
        self.hl_requested.clear();
    }

    pub fn set_collapse_viewed(&mut self, on: bool) {
        if self.collapse_viewed != on {
            self.collapse_viewed = on;
            self.rebuild_anchored();
        }
    }

    /// Syntax colors for a file, if they have arrived.
    pub fn highlight(&self, file: usize) -> Option<&FileHighlight> {
        self.hl_keys
            .get(file)
            .and_then(|k| self.highlights.get(k))
            .map(Arc::as_ref)
    }

    fn is_viewed(&self, i: usize) -> bool {
        self.viewed.get(i).copied().unwrap_or(false)
    }

    /// Folded files show only their header. By default viewed files and
    /// lockfile-like files are folded; `fold` holds explicit choices.
    pub fn is_folded(&self, i: usize) -> bool {
        self.fold
            .get(&self.files[i].path)
            .copied()
            .unwrap_or_else(|| {
                self.files[i].collapse.is_some() || (self.collapse_viewed && self.is_viewed(i))
            })
    }

    /// Recomputes rows from files, patches and fold state.
    pub fn rebuild(&mut self) {
        let mut rows = Vec::new();
        let mut file_rows = Vec::with_capacity(self.files.len());
        let mut max_no = 0u32;
        let row = |file, kind| Row { file, kind };
        for i in 0..self.files.len() {
            if i > 0 {
                rows.push(row(i, RowKind::Separator));
            }
            file_rows.push(rows.len());
            rows.push(row(i, RowKind::FileHeader));
            if self.is_folded(i) {
                // Say why an unviewed file starts folded.
                if self.files[i].collapse.is_some()
                    && !self.is_viewed(i)
                    && !self.fold.contains_key(&self.files[i].path)
                {
                    rows.push(row(i, RowKind::Collapsed));
                }
                continue;
            }
            let f = &self.files[i];
            if f.status == Status::Untracked
                && f.size.unwrap_or(0) > MAX_UNTRACKED
                && !f.is_binary()
            {
                rows.push(row(i, RowKind::TooLarge));
                continue;
            }
            let Some(p) = self.patches.get(i).and_then(Option::as_ref) else {
                rows.push(row(i, RowKind::Loading));
                continue;
            };
            let mut shown = 0;
            for (m, line) in p.meta.iter().enumerate() {
                if shown_meta(line) {
                    rows.push(row(i, RowKind::Meta(m)));
                    shown += 1;
                }
            }
            if p.binary {
                rows.push(row(i, RowKind::Binary));
                continue;
            }
            if p.hunks.is_empty() && shown == 0 {
                rows.push(row(i, RowKind::NoChanges));
            }
            for (h, hunk) in p.hunks.iter().enumerate() {
                if h > 0 {
                    rows.push(row(i, RowKind::Blank));
                }
                rows.push(row(i, RowKind::Hunk(h)));
                max_no = max_no
                    .max(hunk.old_start + hunk.old_len)
                    .max(hunk.new_start + hunk.new_len);
                for l in 0..hunk.lines.len() {
                    rows.push(row(i, RowKind::Line(h, l)));
                }
            }
        }
        self.gutter = max_no.to_string().len().max(3);
        self.rows = rows;
        self.file_rows = file_rows;
        self.clamp_scroll();
    }

    /// Rebuilds rows keeping the view on the same file and offset.
    fn rebuild_anchored(&mut self) {
        let anchor = self.anchor();
        self.rebuild();
        self.restore(anchor);
    }

    /// Folds or unfolds a file and brings its header to the top.
    pub fn toggle_fold(&mut self, i: usize) {
        if i >= self.files.len() {
            return;
        }
        let folded = self.is_folded(i);
        self.fold.insert(self.files[i].path.clone(), !folded);
        self.rebuild();
        self.jump_to_file(i);
    }

    /// Unfolds a file if it is folded.
    pub fn unfold(&mut self, i: usize) {
        if i < self.files.len() && self.is_folded(i) {
            self.toggle_fold(i);
        }
    }

    /// Updates viewed flags. A file that becomes viewed (or unviewed)
    /// drops any explicit fold choice, so marking a file viewed folds it.
    pub fn set_viewed(&mut self, flags: Vec<bool>) {
        if flags == self.viewed {
            return;
        }
        for (i, f) in self.files.iter().enumerate() {
            if flags.get(i) != self.viewed.get(i) {
                self.fold.remove(&f.path);
            }
        }
        self.viewed = flags;
        self.rebuild_anchored();
    }

    /// Replaces files and patches together (a reload), keeping the view on
    /// the same file and offset when the file still exists.
    pub fn replace(
        &mut self,
        spec: DiffSpec,
        files: Vec<FileChange>,
        patches: Vec<Option<FilePatch>>,
        lazy: bool,
    ) {
        let anchor = self.anchor();
        let viewed: HashMap<BString, bool> = self
            .files
            .iter()
            .zip(&self.viewed)
            .map(|(f, v)| (f.path.clone(), *v))
            .collect();
        self.spec = spec;
        self.files = files;
        self.viewed = self
            .files
            .iter()
            .map(|f| viewed.get(&f.path).copied().unwrap_or(false))
            .collect();
        self.set_keys();
        let keys: HashSet<&String> = self.hl_keys.iter().collect();
        self.highlights.retain(|k, _| keys.contains(k));
        // A reload supersedes queued highlight requests (the highlighter
        // drops older ones), so ask again; results are cached by content.
        self.hl_requested.clear();
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
            self.patches[i] = Some(p);
            self.requested.remove(&i);
            self.rebuild_anchored();
        }
    }

    // --- position -------------------------------------------------------

    /// The file at the top of the screen.
    pub fn current_file(&self) -> Option<usize> {
        self.rows
            .get(self.scroll)
            .or(self.rows.last())
            .map(|r| r.file)
    }

    /// The file whose header is pinned to the top line, because its own
    /// header row has scrolled off.
    pub fn sticky(&self) -> Option<usize> {
        let cur = self.current_file()?;
        (self.file_rows[cur] < self.scroll).then_some(cur)
    }

    /// The first row not hidden under the pinned header.
    pub fn reading_row(&self) -> usize {
        self.scroll + usize::from(self.sticky().is_some())
    }

    pub fn anchor(&self) -> Anchor {
        let file = self.current_file()?;
        Some((
            self.files[file].path.to_vec(),
            self.scroll.saturating_sub(self.file_rows[file]),
        ))
    }

    pub fn restore(&mut self, anchor: Anchor) {
        let Some((path, off)) = anchor else {
            return;
        };
        let Some(i) = self.files.iter().position(|f| f.path.as_bytes() == path) else {
            return;
        };
        let start = self.file_rows[i];
        let end = self
            .file_rows
            .get(i + 1)
            .map_or(self.rows.len(), |&r| r - 1);
        self.scroll = (start + off).min(end.saturating_sub(1)).max(start);
        self.clamp_scroll();
    }

    /// The last scroll position: the end of the content, or further so
    /// the last file's header can reach the top.
    pub fn max_scroll(&self) -> usize {
        let end = self.rows.len().saturating_sub(self.viewport.max(1));
        end.max(self.file_rows.last().copied().unwrap_or(0))
    }

    pub fn clamp_scroll(&mut self) {
        self.scroll = self.scroll.min(self.max_scroll());
    }

    pub fn scroll_to(&mut self, row: usize) {
        self.scroll = row;
        self.clamp_scroll();
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let row = (self.scroll as isize + delta).max(0) as usize;
        self.scroll_to(row);
    }

    pub fn top(&mut self) {
        self.scroll = 0;
    }

    pub fn bottom(&mut self) {
        self.scroll = self.max_scroll();
    }

    pub fn jump_to_file(&mut self, i: usize) {
        if let Some(&row) = self.file_rows.get(i) {
            self.scroll_to(row);
        }
    }

    /// Brings the next hunk's `@@` line just below the pinned header.
    pub fn next_hunk(&mut self) -> bool {
        let found = (self.scroll + 2..self.rows.len())
            .find(|&r| matches!(self.rows[r].kind, RowKind::Hunk(_)));
        found.map(|r| self.scroll_to(r - 1)).is_some()
    }

    pub fn prev_hunk(&mut self) -> bool {
        let found = (1..self.scroll + 1)
            .rev()
            .find(|&r| matches!(self.rows[r].kind, RowKind::Hunk(_)) && r - 1 < self.scroll);
        found.map(|r| self.scroll_to(r - 1)).is_some()
    }

    pub fn next_file(&mut self) -> bool {
        let found = self.file_rows.iter().copied().find(|&r| r > self.scroll);
        found.map(|r| self.scroll_to(r)).is_some()
    }

    /// To the current file's header if it is scrolled off, else to the
    /// previous file.
    pub fn prev_file(&mut self) -> bool {
        let found = self
            .file_rows
            .iter()
            .copied()
            .rev()
            .find(|&r| r < self.scroll);
        found.map(|r| self.scroll_to(r)).is_some()
    }

    pub fn scroll_h(&mut self, delta: isize) {
        self.hscroll = (self.hscroll as isize + delta).max(0) as usize;
    }

    fn visible(&self) -> &[Row] {
        let end = (self.scroll + self.viewport.max(1)).min(self.rows.len());
        &self.rows[self.scroll.min(end)..end]
    }

    /// Files on screen that still need loading.
    pub fn visible_unloaded(&self) -> Vec<usize> {
        let mut out = Vec::new();
        for r in self.visible() {
            if r.kind == RowKind::Loading
                && !self.requested.contains(&r.file)
                && !out.contains(&r.file)
            {
                out.push(r.file);
            }
        }
        out
    }

    /// Files on screen, then the next file below (so it is ready by the
    /// time it scrolls into view).
    pub fn visible_files(&self) -> Vec<usize> {
        let mut out: Vec<usize> = Vec::new();
        for r in self.visible() {
            if out.last() != Some(&r.file) {
                out.push(r.file);
            }
        }
        if let Some(&last) = out.last()
            && last + 1 < self.files.len()
        {
            out.push(last + 1);
        }
        out
    }

    /// The line to open in an editor: the first changed line of the
    /// current file on screen, else its first code line on screen, else
    /// its first changed line anywhere.
    pub fn edit_line(&self) -> Option<u32> {
        let cur = self.current_file()?;
        let p = self.patches.get(cur)?.as_ref()?;
        let end = (self.scroll + self.viewport.max(1)).min(self.rows.len());
        let lines = self.rows[self.reading_row().min(end)..end]
            .iter()
            .filter(|r| r.file == cur)
            .filter_map(|r| match r.kind {
                RowKind::Line(h, l) => Some((h, l)),
                _ => None,
            });
        let mut first_code = None;
        for (h, l) in lines {
            let hunk = &p.hunks[h];
            let line = &hunk.lines[l];
            match line.kind {
                LineKind::Add => return line.new,
                LineKind::Del => {
                    // The next new-side line, else the hunk's end.
                    return hunk.lines[l..]
                        .iter()
                        .find_map(|x| x.new)
                        .or(Some(hunk.new_start + hunk.new_len.saturating_sub(1)))
                        .map(|n| n.max(1));
                }
                LineKind::Context => {
                    first_code = first_code.or(line.new);
                }
                LineKind::NoNewline => {}
            }
        }
        first_code.or_else(|| first_changed_line(p))
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
            DiffOpts::default(),
        );
        v.viewport = 5;
        v.set_patches(parsed.into_iter().map(Some).collect(), false);
        v
    }

    #[test]
    fn rows_have_separators_and_hunk_spacing() {
        let v = view();
        let kinds: Vec<RowKind> = v.rows.iter().map(|r| r.kind).collect();
        use RowKind::*;
        assert_eq!(
            kinds,
            [
                FileHeader,
                Hunk(0),
                Line(0, 0),
                Line(0, 1),
                Line(0, 2),
                Line(0, 3),
                Blank,
                Hunk(1),
                Line(1, 0),
                Line(1, 1),
                Line(1, 2),
                Separator,
                FileHeader,
                Hunk(0),
                Line(0, 0),
                Line(0, 1),
            ]
        );
        assert_eq!(v.file_rows, [0, 12]);
        // A separator belongs to the file below it.
        assert_eq!(v.rows[11].file, 1);
    }

    #[test]
    fn pager_scrolling_and_current_file() {
        let mut v = view();
        assert_eq!(v.current_file(), Some(0));
        assert_eq!(v.sticky(), None);
        v.scroll_by(3);
        assert_eq!(v.current_file(), Some(0));
        assert_eq!(v.sticky(), Some(0), "header pinned once scrolled off");
        assert_eq!(v.reading_row(), 4);
        // ] brings the next @@ just below the pinned header.
        assert!(v.next_hunk());
        assert_eq!(v.rows[v.scroll].kind, RowKind::Blank);
        assert_eq!(v.rows[v.reading_row()].kind, RowKind::Hunk(1));
        assert!(v.next_file());
        assert_eq!(v.current_file(), Some(1));
        assert_eq!(v.sticky(), None);
        // The last file's header can reach the top even though it is short.
        assert_eq!(v.scroll, 12);
        assert!(v.prev_file());
        assert_eq!(v.scroll, 0);
        v.bottom();
        assert_eq!(v.current_file(), Some(1));
        v.top();
        assert!(!v.prev_hunk());
    }

    #[test]
    fn edit_line_prefers_changes_on_screen() {
        let mut v = view();
        assert_eq!(v.edit_line(), Some(2), "+Y");
        v.scroll_to(7); // the second hunk's @@ under a pinned header
        assert_eq!(v.edit_line(), Some(11), "+q");
        v.jump_to_file(1);
        assert_eq!(v.edit_line(), Some(5));
    }

    #[test]
    fn anchor_survives_rebuild() {
        let mut v = view();
        v.scroll_to(4);
        let a = v.anchor();
        v.rebuild();
        v.scroll = 0;
        v.restore(a);
        assert_eq!(v.scroll, 4);
    }

    #[test]
    fn folding_and_viewed_files() {
        let mut v = view();
        v.toggle_fold(0);
        assert!(v.is_folded(0));
        assert_eq!(v.rows[0].kind, RowKind::FileHeader);
        assert_eq!(v.rows[1].kind, RowKind::Separator);
        v.toggle_fold(0);
        assert!(!v.is_folded(0));
        // Marking a file viewed folds it, even if it was unfolded by hand…
        v.toggle_fold(1);
        v.toggle_fold(1);
        v.set_viewed(vec![false, true]);
        assert!(v.is_folded(1));
        // …and it can be unfolded again; unmarking unfolds it.
        v.toggle_fold(1);
        assert!(!v.is_folded(1));
        v.set_viewed(vec![false, false]);
        assert!(!v.is_folded(1));
        // With collapse_viewed off, viewed files stay open.
        v.set_collapse_viewed(false);
        v.set_viewed(vec![true, true]);
        assert!(!v.is_folded(0) && !v.is_folded(1));
    }

    #[test]
    fn lockfiles_explain_why_they_are_folded() {
        let mut v = view();
        v.files[0].collapse = Some(crate::model::Collapse::Lockfile);
        v.rebuild();
        assert_eq!(v.rows[1].kind, RowKind::Collapsed);
        v.set_viewed(vec![true, false]);
        assert_eq!(v.rows[1].kind, RowKind::Separator, "viewed: header only");
        v.toggle_fold(0);
        assert_eq!(v.rows[1].kind, RowKind::Hunk(0));
    }
}

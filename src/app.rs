//! Application state and `update(msg) -> effects`. No I/O happens here
//! (marks are only written when main executes `Effect::SaveMarks`).

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::diffview::DiffView;
use crate::git::diff::DiffSpec;
use crate::model::{Commit, FileChange, Status, TargetId};
use crate::msg::{EditRequest, Effect, Msg, RefreshKind, WatchStatus};
use crate::refresh::{HeadChange, RefreshOpts, Snapshot};
use crate::review::{self, Marks};
use crate::ui::palette::Palette;

/// Toasts last 3 s; polling runs every 2 s (in 250 ms ticks).
pub const TOAST_TICKS: u32 = 12;
pub const POLL_TICKS: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Timeline,
    Files,
}

/// Focus inside the diff view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffFocus {
    Diff,
    Explorer,
}

/// Review state of a set of files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    None,
    Some,
    All,
}

impl Glyph {
    pub fn of(viewed: usize, total: usize) -> Glyph {
        if viewed >= total {
            Glyph::All
        } else if viewed == 0 {
            Glyph::None
        } else {
            Glyph::Some
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Glyph::None => "●",
            Glyph::Some => "◐",
            Glyph::All => "✓",
        }
    }
}

pub struct App {
    pub empty_tree: String,
    pub snap: Option<Snapshot>,
    pub error: Option<String>,
    /// Timeline row: 0 is ◌, then commits newest first, then Σ.
    pub sel: usize,
    pub file_sel: usize,
    pub tl_offset: usize,
    pub files_offset: usize,
    pub focus: Focus,
    pub diff: Option<DiffView>,
    pub opts: RefreshOpts,
    pub marks: Marks,
    pub watch: WatchStatus,
    pub toast: Option<(String, u32)>,
    pub help: bool,
    pub size: (u16, u16),
    pub tab_width: usize,
    /// Request syntax highlighting for diff files.
    pub syntax: bool,
    pub palette: Palette,
    /// The file explorer in the diff view (remembered for the session).
    pub explorer_open: bool,
    pub diff_focus: DiffFocus,
    seq: u64,
    applied_gen: u64,
    patch_gen: u64,
    ticks: u32,
    clock: fn() -> i64,
}

fn key_is(k: &KeyEvent, c: char) -> bool {
    k.code == KeyCode::Char(c) && !k.modifiers.contains(KeyModifiers::CONTROL)
}

fn ctrl(k: &KeyEvent, c: char) -> bool {
    k.code == KeyCode::Char(c) && k.modifiers.contains(KeyModifiers::CONTROL)
}

impl App {
    pub fn new(empty_tree: String, marks: Marks, watch: WatchStatus) -> App {
        App {
            empty_tree,
            snap: None,
            error: None,
            sel: 0,
            file_sel: 0,
            tl_offset: 0,
            files_offset: 0,
            focus: Focus::Timeline,
            diff: None,
            opts: RefreshOpts {
                include_wt: true,
                ..RefreshOpts::default()
            },
            marks,
            watch,
            toast: None,
            help: false,
            size: (80, 24),
            tab_width: 4,
            syntax: false,
            palette: Palette::default(),
            explorer_open: false,
            diff_focus: DiffFocus::Diff,
            seq: 0,
            applied_gen: 0,
            patch_gen: 0,
            ticks: 0,
            clock: review::now,
        }
    }

    /// Replace the clock (tests).
    pub fn with_clock(mut self, clock: fn() -> i64) -> Self {
        self.clock = clock;
        self
    }

    pub fn start(&mut self) -> Vec<Effect> {
        vec![self.refresh(RefreshKind::Full)]
    }

    fn refresh(&mut self, kind: RefreshKind) -> Effect {
        self.seq += 1;
        Effect::Refresh {
            seq: self.seq,
            kind,
            opts: self.opts.clone(),
        }
    }

    fn toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), TOAST_TICKS));
    }

    // --- queries used by the UI -------------------------------------------

    pub fn commits(&self) -> &[Commit] {
        self.snap
            .as_ref()
            .map(|s| s.commits.as_slice())
            .unwrap_or_default()
    }

    pub fn has_total(&self) -> bool {
        self.snap.as_ref().is_some_and(|s| s.total.is_some())
    }

    pub fn row_count(&self) -> usize {
        1 + self.commits().len() + usize::from(self.has_total())
    }

    pub fn sigma_row(&self) -> Option<usize> {
        self.has_total().then(|| 1 + self.commits().len())
    }

    pub fn target_at(&self, row: usize) -> Option<TargetId> {
        let n = self.commits().len();
        match row {
            0 => Some(TargetId::Uncommitted),
            r if r <= n => Some(TargetId::Commit(self.commits()[r - 1].sha.clone())),
            r if r == n + 1 && self.has_total() => Some(TargetId::Total),
            _ => None,
        }
    }

    pub fn row_of(&self, id: &TargetId) -> Option<usize> {
        match id {
            TargetId::Uncommitted => Some(0),
            TargetId::Total => self.sigma_row(),
            TargetId::Commit(sha) => self
                .commits()
                .iter()
                .position(|c| &c.sha == sha)
                .map(|i| i + 1),
        }
    }

    pub fn selected(&self) -> Option<TargetId> {
        self.target_at(self.sel)
    }

    pub fn files_of(&self, id: &TargetId) -> &[FileChange] {
        self.snap.as_ref().map(|s| s.files(id)).unwrap_or_default()
    }

    pub fn selected_files(&self) -> &[FileChange] {
        match self.selected() {
            Some(id) => self.files_of(&id),
            None => &[],
        }
    }

    pub fn is_viewed(&self, f: &FileChange) -> bool {
        self.marks.is_viewed(&f.mark_key())
    }

    pub fn viewed_count(&self, files: &[FileChange]) -> usize {
        files.iter().filter(|f| self.is_viewed(f)).count()
    }

    pub fn glyph(&self, files: &[FileChange]) -> Glyph {
        Glyph::of(self.viewed_count(files), files.len())
    }

    /// Commits with at least one unviewed file.
    pub fn to_review(&self) -> usize {
        self.commits()
            .iter()
            .filter(|c| self.glyph(&c.files) != Glyph::All)
            .count()
    }

    fn spec_of(&self, id: &TargetId) -> Option<DiffSpec> {
        self.snap.as_ref()?.spec(id, &self.empty_tree)
    }

    // --- update -----------------------------------------------------------

    pub fn update(&mut self, msg: Msg) -> Vec<Effect> {
        let fx = self.handle(msg);
        // Marks may have changed (keys, reloads); viewed files fold.
        self.sync_viewed();
        fx
    }

    fn handle(&mut self, msg: Msg) -> Vec<Effect> {
        match msg {
            Msg::Key(k) => {
                if k.kind != KeyEventKind::Press {
                    return Vec::new();
                }
                if self.help {
                    self.help = false;
                    return Vec::new();
                }
                if ctrl(&k, 'c') {
                    return vec![Effect::Quit];
                }
                let mut fx = if self.diff.is_some() {
                    self.diff_key(k)
                } else {
                    self.main_key(k)
                };
                fx.extend(self.view_effects());
                fx
            }
            Msg::Resize(w, h) => {
                self.size = (w, h);
                // A drawer only makes sense while it has focus.
                if self.narrow() && self.diff_focus == DiffFocus::Diff {
                    self.explorer_open = false;
                }
                if let Some(d) = &mut self.diff {
                    d.viewport = diff_viewport(h);
                    d.clamp_scroll();
                }
                self.view_effects()
            }
            Msg::Fs(kind) => vec![self.refresh(kind)],
            Msg::Watch(status) => {
                self.watch = status;
                Vec::new()
            }
            Msg::Refreshed { seq, result } => {
                if seq < self.applied_gen {
                    return Vec::new();
                }
                self.applied_gen = seq;
                match result {
                    Ok(snap) => {
                        self.error = None;
                        let mut fx = self.apply_snapshot(*snap);
                        fx.extend(self.view_effects());
                        fx
                    }
                    Err(e) => {
                        self.error = Some(e);
                        Vec::new()
                    }
                }
            }
            Msg::PatchLoaded { seq, result } => {
                if seq != self.patch_gen {
                    return Vec::new();
                }
                let Some(d) = &mut self.diff else {
                    return Vec::new();
                };
                match result {
                    Ok(p) => {
                        let spec = d.spec.clone();
                        d.error = None;
                        d.replace(spec, p.files, p.patches, p.lazy);
                    }
                    Err(e) => {
                        d.loading = false;
                        d.error = Some(e);
                    }
                }
                self.view_effects()
            }
            Msg::FileLoaded { seq, index, result } => {
                if seq != self.patch_gen {
                    return Vec::new();
                }
                if let Some(d) = &mut self.diff {
                    match result {
                        Ok(p) => d.set_file_patch(index, p),
                        Err(e) => {
                            d.requested.remove(&index);
                            d.error = Some(e);
                        }
                    }
                }
                self.view_effects()
            }
            Msg::Highlighted { key, hl, .. } => {
                if let Some(d) = &mut self.diff {
                    d.hl_requested.remove(&key);
                    if d.keys.contains(&key) {
                        d.highlights.insert(key, hl);
                    }
                }
                Vec::new()
            }
            Msg::Tick => {
                self.ticks = self.ticks.wrapping_add(1);
                if let Some((_, left)) = &mut self.toast {
                    *left = left.saturating_sub(1);
                    if *left == 0 {
                        self.toast = None;
                    }
                }
                if self.watch != WatchStatus::Live && self.ticks % POLL_TICKS == 0 {
                    return vec![self.refresh(RefreshKind::Full)];
                }
                Vec::new()
            }
        }
    }

    /// Below `ui::WIDE` columns the explorer is a drawer.
    pub fn narrow(&self) -> bool {
        self.size.0 < crate::ui::WIDE
    }

    /// Loads and highlighting for whatever the diff viewport shows.
    fn view_effects(&mut self) -> Vec<Effect> {
        let mut fx = self.lazy_loads();
        fx.extend(self.highlights());
        fx
    }

    /// Syntax highlighting for loaded files in (or just below) the
    /// viewport.
    fn highlights(&mut self) -> Vec<Effect> {
        let seq = self.patch_gen;
        let Some(d) = &mut self.diff else {
            return Vec::new();
        };
        if !self.syntax || d.loading {
            return Vec::new();
        }
        let mut fx = Vec::new();
        for i in d.visible_files() {
            let Some(p) = &d.patches[i] else { continue };
            let key = &d.keys[i];
            if p.binary
                || p.hunks.is_empty()
                || d.is_folded(i)
                || d.highlights.contains_key(key)
                || d.hl_requested.contains(key)
            {
                continue;
            }
            d.hl_requested.insert(key.clone());
            fx.push(Effect::Highlight {
                seq,
                index: i,
                key: key.clone(),
                path: d.files[i].path.clone(),
                patch: p.clone(),
            });
        }
        fx
    }

    /// Lazy file loads for whatever the diff viewport shows.
    fn lazy_loads(&mut self) -> Vec<Effect> {
        let seq = self.patch_gen;
        let Some(d) = &mut self.diff else {
            return Vec::new();
        };
        if d.loading {
            return Vec::new();
        }
        let mut fx = Vec::new();
        for i in d.visible_unloaded() {
            d.requested.insert(i);
            fx.push(Effect::LoadFile {
                seq,
                index: i,
                spec: d.spec.clone(),
                file: d.files[i].clone(),
            });
        }
        fx
    }

    fn apply_snapshot(&mut self, new: Snapshot) -> Vec<Effect> {
        let prev_target = self.selected();
        let prev_commit = match &prev_target {
            Some(TargetId::Commit(sha)) => self
                .snap
                .as_ref()
                .and_then(|s| s.commit(sha))
                .map(|c| (c.subject.clone(), c.author.clone())),
            _ => None,
        };
        let prev_file = self
            .selected_files()
            .get(self.file_sel)
            .map(|f| f.path.clone());
        let diff_commit = match self.diff.as_ref().map(|d| &d.target) {
            Some(TargetId::Commit(sha)) => self
                .snap
                .as_ref()
                .and_then(|s| s.commit(sha))
                .map(|c| (c.subject.clone(), c.author.clone())),
            _ => None,
        };
        let change = new.change.clone();
        self.snap = Some(new);

        self.sel = self.relocate(prev_target.as_ref(), prev_commit.as_ref(), self.sel);
        let files = self.selected_files();
        self.file_sel = prev_file
            .and_then(|p| files.iter().position(|f| f.path == p))
            .unwrap_or(self.file_sel)
            .min(files.len().saturating_sub(1));

        match change {
            Some(HeadChange::Added(n)) => {
                self.toast(format!("+{n} commit{}", if n == 1 { "" } else { "s" }))
            }
            Some(HeadChange::Rewritten) => self.toast("history rewritten (amend/rebase)"),
            Some(HeadChange::Switched(b)) => self.toast(format!(
                "switched to {}",
                b.as_deref().unwrap_or("detached HEAD")
            )),
            None => {}
        }

        // Keep the open diff in step with the new snapshot.
        let Some(d) = &self.diff else {
            return Vec::new();
        };
        let target = d.target.clone();
        let mapped = match &target {
            TargetId::Commit(_) if self.row_of(&target).is_none() => {
                self.relocate_commit(diff_commit.as_ref())
            }
            _ => self.row_of(&target).map(|_| target.clone()),
        };
        let Some(mapped) = mapped else {
            self.diff = None;
            self.focus = Focus::Timeline;
            self.toast("target is gone");
            return Vec::new();
        };
        let Some(spec) = self.spec_of(&mapped) else {
            return Vec::new();
        };
        let files = self.files_of(&mapped).to_vec();
        let d = self.diff.as_mut().expect("diff open");
        if mapped == d.target && spec == d.spec && files == d.files {
            return Vec::new();
        }
        d.target = mapped;
        d.spec = spec.clone();
        self.patch_gen += 1;
        vec![Effect::LoadPatch {
            seq: self.patch_gen,
            spec,
            files,
        }]
    }

    /// Selection stability (PLAN §6.5): same target, else same subject and
    /// author, else same position, clamped.
    fn relocate(
        &self,
        prev: Option<&TargetId>,
        commit: Option<&(String, String)>,
        old_row: usize,
    ) -> usize {
        let last = self.row_count() - 1;
        if let Some(r) = prev.and_then(|t| self.row_of(t)) {
            return r;
        }
        if let Some(TargetId::Total) = prev {
            return last;
        }
        if let Some(r) = commit
            .and_then(|c| self.relocate_commit(Some(c)))
            .and_then(|t| self.row_of(&t))
        {
            return r;
        }
        old_row.min(last)
    }

    fn relocate_commit(&self, info: Option<&(String, String)>) -> Option<TargetId> {
        let (subject, author) = info?;
        self.commits()
            .iter()
            .find(|c| &c.subject == subject && &c.author == author)
            .map(|c| TargetId::Commit(c.sha.clone()))
    }

    // --- navigation helpers -----------------------------------------------

    fn select(&mut self, row: usize) {
        let row = row.min(self.row_count() - 1);
        if row != self.sel {
            self.sel = row;
            self.file_sel = 0;
            self.files_offset = 0;
        }
    }

    /// Row of the next newer (`-1`) or older (`+1`) commit.
    fn step_commit(&self, from: usize, dir: isize) -> Option<usize> {
        let n = self.commits().len();
        if n == 0 {
            return None;
        }
        let row = match (from, dir) {
            (0, d) if d > 0 => 1,
            (0, _) => return None,
            (r, d) if r > n => {
                if d < 0 {
                    n
                } else {
                    return None;
                }
            }
            (r, d) => {
                let next = r as isize + d;
                if next < 1 || next > n as isize {
                    return None;
                }
                next as usize
            }
        };
        Some(row)
    }

    /// Row of the oldest commit with unviewed files.
    fn oldest_unreviewed(&self) -> Option<usize> {
        let commits = self.commits();
        (0..commits.len())
            .rev()
            .find(|&i| self.glyph(&commits[i].files) != Glyph::All)
            .map(|i| i + 1)
    }

    fn first_unviewed(&self, files: &[FileChange]) -> Option<usize> {
        files.iter().position(|f| !self.is_viewed(f))
    }

    fn set_viewed(&mut self, f: &FileChange, viewed: bool) {
        let now = (self.clock)();
        self.marks.set(f.mark_key(), viewed, now);
    }

    /// Tells the open diff which files are viewed, so they fold.
    pub fn sync_viewed(&mut self) {
        let Some(d) = &self.diff else { return };
        let flags: Vec<bool> = d.keys.iter().map(|k| self.marks.is_viewed(k)).collect();
        if let Some(d) = self.diff.as_mut() {
            d.set_viewed(flags);
        }
    }

    fn toggle_target_viewed(&mut self) -> Vec<Effect> {
        let files = self.selected_files().to_vec();
        if files.is_empty() {
            return Vec::new();
        }
        let all = self.glyph(&files) == Glyph::All;
        for f in &files {
            self.set_viewed(f, !all);
        }
        vec![Effect::SaveMarks]
    }

    fn open_diff(&mut self, id: TargetId, file: Option<usize>) -> Vec<Effect> {
        let Some(spec) = self.spec_of(&id) else {
            return Vec::new();
        };
        let files = self.files_of(&id).to_vec();
        if files.is_empty() {
            self.toast("no changes");
            return Vec::new();
        }
        if let Some(r) = self.row_of(&id) {
            self.select(r);
        }
        let mut view = DiffView::new(id, spec.clone(), files.clone());
        view.viewport = diff_viewport(self.size.1);
        if let Some(i) = file {
            view.jump_to_file(i);
            self.file_sel = i;
        }
        self.diff = Some(view);
        self.diff_focus = DiffFocus::Diff;
        self.patch_gen += 1;
        vec![Effect::LoadPatch {
            seq: self.patch_gen,
            spec,
            files,
        }]
    }

    fn close_diff(&mut self) {
        self.diff_focus = DiffFocus::Diff;
        if let Some(d) = self.diff.take() {
            if let Some(r) = self.row_of(&d.target) {
                self.sel = r;
            }
            self.file_sel = d.current_file().unwrap_or(0);
            self.focus = Focus::Files;
        }
    }

    fn toggle_merge_mode(&mut self, id: Option<TargetId>) -> Vec<Effect> {
        let Some(TargetId::Commit(sha)) = id else {
            return Vec::new();
        };
        let is_merge = self
            .snap
            .as_ref()
            .and_then(|s| s.commit(&sha))
            .is_some_and(Commit::is_merge);
        if !is_merge {
            return Vec::new();
        }
        if !self.snap.as_ref().is_some_and(|s| s.remerge) {
            self.toast("remerge-diff needs git 2.36+ · showing first-parent diff");
            return Vec::new();
        }
        if !self.opts.first_parent.remove(&sha) {
            self.opts.first_parent.insert(sha);
            self.toast("merge: first-parent diff");
        } else {
            self.toast("merge: remerge-diff");
        }
        vec![self.refresh(RefreshKind::Full)]
    }

    fn edit(&mut self, id: TargetId, file: FileChange, line: Option<u32>) -> Vec<Effect> {
        if file.status == Status::Deleted {
            self.toast("file was deleted · nothing to open");
            return Vec::new();
        }
        let Some(spec) = self.spec_of(&id) else {
            return Vec::new();
        };
        let commit = match &id {
            TargetId::Commit(sha) => Some(sha[..sha.len().min(7)].to_owned()),
            _ => None,
        };
        vec![Effect::OpenEditor(EditRequest {
            file,
            line,
            spec,
            commit,
        })]
    }

    /// Keys that work the same everywhere. `None` means "not handled".
    fn global_key(&mut self, k: &KeyEvent) -> Option<Vec<Effect>> {
        if ctrl(k, 'l') {
            return Some(vec![Effect::Redraw, self.refresh(RefreshKind::Full)]);
        }
        if key_is(k, '?') {
            self.help = true;
            return Some(Vec::new());
        }
        if key_is(k, 'i') {
            self.opts.include_wt = !self.opts.include_wt;
            self.toast(if self.opts.include_wt {
                "Σ includes uncommitted changes"
            } else {
                "Σ shows committed changes only"
            });
            return Some(vec![self.refresh(RefreshKind::Worktree)]);
        }
        None
    }

    fn main_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        if let Some(fx) = self.global_key(&k) {
            return fx;
        }
        let files_len = self.selected_files().len();
        match k.code {
            KeyCode::Char('q') => return vec![Effect::Quit],
            KeyCode::Esc => self.focus = Focus::Timeline,
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Timeline => Focus::Files,
                    Focus::Files => Focus::Timeline,
                }
            }
            KeyCode::Char('w') => self.select(0),
            KeyCode::Char('b') => {
                if let Some(r) = self.sigma_row() {
                    self.select(r)
                }
            }
            KeyCode::Char('u') => match self.oldest_unreviewed() {
                Some(r) => {
                    // Focus the first unviewed file, so Enter opens it.
                    self.select(r);
                    let files = self.selected_files();
                    self.file_sel = self.first_unviewed(files).unwrap_or(0);
                    self.focus = Focus::Files;
                    return Vec::new();
                }
                None => self.toast("nothing left to review ✓"),
            },
            KeyCode::Char('n') => {
                if let Some(r) = self.step_commit(self.sel, -1) {
                    self.select(r)
                }
            }
            KeyCode::Char('p') => {
                if let Some(r) = self.step_commit(self.sel, 1) {
                    self.select(r)
                }
            }
            KeyCode::Char('m') => return self.toggle_merge_mode(self.selected()),
            KeyCode::Char('r') => return self.toggle_target_viewed(),
            _ => {}
        }
        match self.focus {
            Focus::Timeline => {
                let last = self.row_count() - 1;
                match k.code {
                    KeyCode::Char('j') | KeyCode::Down => self.select((self.sel + 1).min(last)),
                    KeyCode::Char('k') | KeyCode::Up => self.select(self.sel.saturating_sub(1)),
                    KeyCode::Char('g') | KeyCode::Home => self.select(0),
                    KeyCode::Char('G') | KeyCode::End => self.select(last),
                    KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                        if files_len > 0 {
                            self.focus = Focus::Files;
                        } else {
                            self.toast("no changes");
                        }
                    }
                    _ => {}
                }
            }
            Focus::Files => {
                let last = files_len.saturating_sub(1);
                match k.code {
                    KeyCode::Char('j') | KeyCode::Down => {
                        self.file_sel = (self.file_sel + 1).min(last)
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.file_sel = self.file_sel.saturating_sub(1)
                    }
                    KeyCode::Char('g') | KeyCode::Home => self.file_sel = 0,
                    KeyCode::Char('G') | KeyCode::End => self.file_sel = last,
                    KeyCode::Left | KeyCode::Char('h') => self.focus = Focus::Timeline,
                    KeyCode::Enter => {
                        if let Some(id) = self.selected() {
                            if files_len > 0 {
                                return self.open_diff(id, Some(self.file_sel));
                            }
                        }
                    }
                    KeyCode::Char(' ') => {
                        if let Some(f) = self.selected_files().get(self.file_sel).cloned() {
                            let v = self.is_viewed(&f);
                            self.set_viewed(&f, !v);
                            return vec![Effect::SaveMarks];
                        }
                    }
                    KeyCode::Char('e') => {
                        if let (Some(id), Some(f)) = (
                            self.selected(),
                            self.selected_files().get(self.file_sel).cloned(),
                        ) {
                            return self.edit(id, f, None);
                        }
                    }
                    _ => {}
                }
            }
        }
        Vec::new()
    }

    /// Toggles the explorer; opening it gives it focus.
    fn toggle_explorer(&mut self) {
        if self.explorer_open {
            self.explorer_open = false;
            self.diff_focus = DiffFocus::Diff;
        } else {
            self.explorer_open = true;
            self.focus_explorer();
        }
    }

    fn focus_explorer(&mut self) {
        self.diff_focus = DiffFocus::Explorer;
    }

    /// Hands focus back to the diff; a drawer closes.
    fn leave_explorer(&mut self) {
        self.diff_focus = DiffFocus::Diff;
        if self.narrow() {
            self.explorer_open = false;
        }
    }

    /// The explorer always marks the diff's current file; moving in it
    /// moves the diff.
    fn explorer_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        let Some(d) = self.diff.as_mut() else {
            return Vec::new();
        };
        let last = d.files.len().saturating_sub(1);
        let cur = d.current_file().unwrap_or(0);
        match k.code {
            KeyCode::Char('j') | KeyCode::Down => d.jump_to_file((cur + 1).min(last)),
            KeyCode::Char('k') | KeyCode::Up => d.jump_to_file(cur.saturating_sub(1)),
            KeyCode::Char('g') | KeyCode::Home => d.jump_to_file(0),
            KeyCode::Char('G') | KeyCode::End => d.jump_to_file(last),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                // Picking a file means you want to read it.
                d.unfold(cur);
                self.leave_explorer();
            }
            KeyCode::Char(' ') => {
                if let Some(f) = d.files.get(cur).cloned() {
                    let v = self.is_viewed(&f);
                    self.set_viewed(&f, !v);
                    self.sync_viewed();
                    if let Some(d) = self.diff.as_mut() {
                        d.jump_to_file(cur);
                    }
                    return vec![Effect::SaveMarks];
                }
            }
            KeyCode::Char('e') => {
                if let Some(f) = d.files.get(cur).cloned() {
                    let id = d.target.clone();
                    return self.edit(id, f, None);
                }
            }
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('h') | KeyCode::Left => {
                self.leave_explorer()
            }
            // Everything else (n/p, w/b, u, m…) works as in the diff.
            _ => {
                self.diff_focus = DiffFocus::Diff;
                let fx = self.diff_key(k);
                if self.explorer_open && self.diff.is_some() {
                    self.focus_explorer();
                }
                return fx;
            }
        }
        Vec::new()
    }

    fn diff_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        if let Some(fx) = self.global_key(&k) {
            return fx;
        }
        if key_is(&k, 'f') {
            self.toggle_explorer();
            return Vec::new();
        }
        if self.explorer_open && matches!(k.code, KeyCode::Tab | KeyCode::BackTab) {
            match self.diff_focus {
                DiffFocus::Diff => self.focus_explorer(),
                DiffFocus::Explorer => self.leave_explorer(),
            }
            return Vec::new();
        }
        if self.explorer_open && self.diff_focus == DiffFocus::Explorer {
            return self.explorer_key(k);
        }
        let Some(d) = self.diff.as_mut() else {
            return Vec::new();
        };
        let half = (d.viewport / 2).max(1) as isize;
        let page = d.viewport.saturating_sub(2).max(1) as isize;
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => self.close_diff(),
            KeyCode::Char('j') | KeyCode::Down => d.scroll_by(1),
            KeyCode::Char('k') | KeyCode::Up => d.scroll_by(-1),
            KeyCode::Char('d') if ctrl(&k, 'd') => d.scroll_by(half),
            KeyCode::Char('u') if ctrl(&k, 'u') => d.scroll_by(-half),
            KeyCode::PageDown => d.scroll_by(page),
            KeyCode::PageUp => d.scroll_by(-page),
            KeyCode::Char('g') | KeyCode::Home => d.top(),
            KeyCode::Char('G') | KeyCode::End => d.bottom(),
            KeyCode::Char(']') => {
                d.next_hunk();
            }
            KeyCode::Char('[') => {
                d.prev_hunk();
            }
            KeyCode::Char('}') => {
                d.next_file();
            }
            KeyCode::Char('{') => {
                d.prev_file();
            }
            KeyCode::Char('h') | KeyCode::Left => d.scroll_h(-8),
            KeyCode::Char('l') | KeyCode::Right => d.scroll_h(8),
            KeyCode::Enter | KeyCode::Char('o') => {
                if let Some(i) = d.current_file() {
                    d.toggle_fold(i);
                }
            }
            KeyCode::Char(' ') => return self.view_and_advance(),
            KeyCode::Char('m') => {
                let id = d.target.clone();
                return self.toggle_merge_mode(Some(id));
            }
            KeyCode::Char('e') => {
                let (Some(i), line) = (d.current_file(), d.edit_line()) else {
                    return Vec::new();
                };
                let (id, f) = (d.target.clone(), d.files[i].clone());
                return self.edit(id, f, line);
            }
            KeyCode::Char('n') | KeyCode::Char('p') => {
                let dir = if k.code == KeyCode::Char('n') { -1 } else { 1 };
                let target = d.target.clone();
                let from = self.row_of(&target).unwrap_or(self.sel);
                match self.step_commit(from, dir).and_then(|r| self.target_at(r)) {
                    Some(id) => return self.open_diff(id, Some(0)),
                    None => self.toast(if dir < 0 {
                        "no newer commit"
                    } else {
                        "no older commit"
                    }),
                }
            }
            KeyCode::Char('w') => return self.open_diff(TargetId::Uncommitted, Some(0)),
            KeyCode::Char('b') => {
                if self.has_total() {
                    return self.open_diff(TargetId::Total, Some(0));
                }
            }
            KeyCode::Char('u') => match self.oldest_unreviewed().and_then(|r| self.target_at(r)) {
                Some(id) => {
                    let first = self.first_unviewed(self.files_of(&id)).unwrap_or(0);
                    return self.open_diff(id, Some(first));
                }
                None => self.toast("nothing left to review ✓"),
            },
            _ => {}
        }
        Vec::new()
    }

    /// Space in the diff view: mark the current file viewed (which folds
    /// it), then jump to the next unviewed file, continuing into the next
    /// commit. On a viewed file, unmark it (which unfolds it) instead.
    fn view_and_advance(&mut self) -> Vec<Effect> {
        let Some(d) = &self.diff else {
            return Vec::new();
        };
        let Some(cur) = d.current_file() else {
            return Vec::new();
        };
        let file = d.files[cur].clone();
        let target = d.target.clone();
        let fx = vec![Effect::SaveMarks];
        if self.is_viewed(&file) {
            self.set_viewed(&file, false);
            self.sync_viewed();
            if let Some(d) = self.diff.as_mut() {
                d.jump_to_file(cur);
            }
            return fx;
        }
        self.set_viewed(&file, true);
        self.sync_viewed();
        let mut fx = fx;

        let d = self.diff.as_ref().expect("diff open");
        let n = d.files.len();
        let next = (cur + 1..n)
            .chain(0..cur)
            .find(|&i| !self.is_viewed(&d.files[i]));
        if let Some(i) = next {
            self.diff.as_mut().expect("diff open").jump_to_file(i);
            return fx;
        }
        if let TargetId::Commit(_) = target {
            let from = self.row_of(&target).unwrap_or(1);
            let commits = self.commits();
            let newer = (1..from)
                .rev()
                .find(|&r| self.glyph(&commits[r - 1].files) != Glyph::All);
            if let Some(row) = newer.or_else(|| self.oldest_unreviewed()) {
                let id = self.target_at(row).expect("commit row");
                let first = self.first_unviewed(self.files_of(&id)).unwrap_or(0);
                fx.extend(self.open_diff(id, Some(first)));
                return fx;
            }
            self.toast("all commits reviewed ✓");
        } else {
            self.toast("all files viewed ✓");
        }
        fx
    }
}

/// Diff content height: the screen minus header, two rules and footer.
pub fn diff_viewport(height: u16) -> usize {
    height.saturating_sub(4).max(1) as usize
}

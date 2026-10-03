//! Messages into the app, and effects out of it.

use std::sync::Arc;

use bstr::BString;
use crossterm::event::KeyEvent;

use crate::git::diff::DiffSpec;
use crate::highlight::FileHighlight;
use crate::model::{FileChange, FilePatch};
use crate::refresh::{LoadedPatch, RefreshOpts, Snapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefreshKind {
    /// Working tree or index changed: ◌, Σ and the open volatile diff.
    Worktree,
    /// HEAD or refs changed: everything, including base resolution.
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchStatus {
    Live,
    Polling,
    Error(String),
}

#[derive(Debug)]
pub enum Msg {
    Key(KeyEvent),
    Resize(u16, u16),
    /// A debounced file-system change.
    Fs(RefreshKind),
    Watch(WatchStatus),
    Refreshed {
        seq: u64,
        result: Result<Box<Snapshot>, String>,
    },
    PatchLoaded {
        seq: u64,
        result: Result<LoadedPatch, String>,
    },
    FileLoaded {
        seq: u64,
        index: usize,
        result: Result<FilePatch, String>,
    },
    /// Syntax colors for one file, keyed by its content key.
    Highlighted {
        index: usize,
        key: String,
        hl: Arc<FileHighlight>,
    },
    /// Every 250 ms.
    Tick,
}

/// A request to open a file in the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditRequest {
    pub file: FileChange,
    /// `None`: find the first changed line.
    pub line: Option<u32>,
    pub spec: DiffSpec,
    /// Short sha when the file comes from a historical commit.
    pub commit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Refresh {
        seq: u64,
        kind: RefreshKind,
        opts: RefreshOpts,
    },
    LoadPatch {
        seq: u64,
        spec: DiffSpec,
        files: Vec<FileChange>,
    },
    LoadFile {
        seq: u64,
        index: usize,
        spec: DiffSpec,
        file: FileChange,
    },
    /// Syntax-highlight one loaded file of the open diff.
    Highlight {
        seq: u64,
        index: usize,
        key: String,
        path: BString,
        patch: FilePatch,
    },
    SaveMarks,
    OpenEditor(EditRequest),
    /// Clear and redraw the whole screen.
    Redraw,
    Quit,
}

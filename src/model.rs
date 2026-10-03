//! Review targets, file changes and parsed patches.

use bstr::{BString, ByteSlice};

/// Identifies a row of the timeline independently of its position.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TargetId {
    Uncommitted,
    Commit(String),
    Total,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Modified,
    Added,
    Deleted,
    Renamed(u8),
    Copied(u8),
    TypeChange,
    Unmerged,
    Untracked,
    Unknown,
}

impl Status {
    pub fn letter(self) -> char {
        match self {
            Status::Modified => 'M',
            Status::Added => 'A',
            Status::Deleted => 'D',
            Status::Renamed(_) => 'R',
            Status::Copied(_) => 'C',
            Status::TypeChange => 'T',
            Status::Unmerged => 'U',
            Status::Untracked => '?',
            Status::Unknown => 'X',
        }
    }

    /// Parses the status field of `--raw` output (`M`, `R092`, …).
    pub fn from_raw(s: &[u8]) -> Status {
        let score = || {
            s.get(1..)
                .and_then(|d| d.to_str().ok())
                .and_then(|d| d.parse::<u8>().ok())
                .unwrap_or(0)
        };
        match s.first() {
            Some(b'M') => Status::Modified,
            Some(b'A') => Status::Added,
            Some(b'D') => Status::Deleted,
            Some(b'R') => Status::Renamed(score()),
            Some(b'C') => Status::Copied(score()),
            Some(b'T') => Status::TypeChange,
            Some(b'U') => Status::Unmerged,
            _ => Status::Unknown,
        }
    }
}

pub const MODE_SYMLINK: u32 = 0o120000;
pub const MODE_GITLINK: u32 = 0o160000;

/// Why a file starts collapsed in the diff view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Collapse {
    Lockfile,
    Generated,
    NoDiff,
    Large,
}

impl Collapse {
    pub fn label(self) -> &'static str {
        match self {
            Collapse::Lockfile => "lockfile",
            Collapse::Generated => "generated",
            Collapse::NoDiff => "-diff",
            Collapse::Large => "large",
        }
    }
}

/// One changed file within a target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: BString,
    /// Source path for renames and copies.
    pub old_path: Option<BString>,
    pub status: Status,
    pub old_mode: u32,
    pub new_mode: u32,
    pub old_oid: String,
    /// For working-tree files this is filled in by hashing the file.
    pub new_oid: String,
    /// `None` for binary files.
    pub added: Option<u64>,
    pub deleted: Option<u64>,
    pub collapse: Option<Collapse>,
    /// Untracked file size, used for the summary of big files.
    pub size: Option<u64>,
}

impl FileChange {
    pub fn is_binary(&self) -> bool {
        self.added.is_none() && self.status != Status::Unmerged
    }

    pub fn is_submodule(&self) -> bool {
        self.old_mode == MODE_GITLINK || self.new_mode == MODE_GITLINK
    }

    pub fn lines_changed(&self) -> u64 {
        self.added.unwrap_or(0) + self.deleted.unwrap_or(0)
    }

    pub fn display_path(&self) -> String {
        self.path.to_str_lossy().into_owned()
    }

    /// The content key used for viewed marks (PLAN §6.3).
    pub fn mark_key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.old_oid,
            self.new_oid,
            self.path.to_str_lossy()
        )
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub files: usize,
    pub added: u64,
    pub deleted: u64,
}

impl Stats {
    pub fn of(files: &[FileChange]) -> Stats {
        Stats {
            files: files.len(),
            added: files.iter().filter_map(|f| f.added).sum(),
            deleted: files.iter().filter_map(|f| f.deleted).sum(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub time: i64,
    pub subject: String,
    pub files: Vec<FileChange>,
}

impl Commit {
    pub fn short(&self) -> &str {
        &self.sha[..self.sha.len().min(7)]
    }

    pub fn is_merge(&self) -> bool {
        self.parents.len() > 1
    }
}

// ---------------------------------------------------------------------------
// Parsed unified diffs

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Add,
    Del,
    /// `\ No newline at end of file`
    NoNewline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub kind: LineKind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: BString,
    /// Byte ranges of changed words (see `worddiff`).
    pub emph: Vec<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: u32,
    pub old_len: u32,
    pub new_start: u32,
    pub new_len: u32,
    /// The whole `@@ … @@ section` line.
    pub header: BString,
    pub lines: Vec<Line>,
}

/// The patch of one file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilePatch {
    /// The `diff --git` line, used to pair the patch with its file.
    pub header: BString,
    /// Extended header lines (mode changes, similarity, …), minus `index`.
    pub meta: Vec<BString>,
    pub binary: bool,
    pub hunks: Vec<Hunk>,
    /// Blob sizes, filled in for binary files.
    pub old_size: Option<u64>,
    pub new_size: Option<u64>,
}

impl FilePatch {
    pub fn line_count(&self) -> usize {
        self.hunks.iter().map(|h| h.lines.len() + 1).sum()
    }
}

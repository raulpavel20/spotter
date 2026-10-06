//! Messages into the app, and effects out of it.

use std::sync::Arc;
use std::sync::mpsc::Sender;

use bstr::BString;
use crossterm::event::KeyEvent;

use crate::askpass::PromptKind;
use crate::config::{Config, Loaded, Value};
use crate::git::diff::{DiffOpts, DiffSpec};
use crate::git::write::{CommitFile, PushInfo};
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

/// One repository's tab when Spotter shows several.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(pub u32);

#[derive(Debug)]
pub enum Msg {
    /// A message about one repository's tab.
    Tab(TabId, Box<Msg>),
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
    /// Settings re-read after editing the file.
    ConfigReloaded(Box<Loaded>),
    /// git or ssh asks for a password (or a username, or a yes).
    AskPass {
        id: u64,
        prompt: String,
        kind: PromptKind,
    },
    /// A commit finished: its short id, or what went wrong.
    Committed(Result<String, String>),
    /// What a push would do, for the confirm panel.
    PushInfo(Result<PushInfo, String>),
    /// A push finished: what happened, or what went wrong.
    Pushed(Result<String, String>),
    /// The commit message, back from the editor.
    MessageEdited(String),
    /// Text pasted into the terminal (bracketed paste).
    Paste(String),
    /// Every 250 ms.
    Tick,
}

/// Where background threads send messages: tagged with their tab when
/// they work for one repository of several.
#[derive(Debug, Clone)]
pub struct Outbox {
    tx: Sender<Msg>,
    tab: Option<TabId>,
}

impl Outbox {
    pub fn tagged(tx: Sender<Msg>, tab: TabId) -> Self {
        Outbox { tx, tab: Some(tab) }
    }

    pub fn tab(&self) -> Option<TabId> {
        self.tab
    }

    pub fn send(&self, msg: Msg) -> Result<(), Closed> {
        self.tx.send(tag(self.tab, msg)).map_err(|_| Closed)
    }
}

/// The app is gone; nobody reads messages any more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Closed;

impl From<Sender<Msg>> for Outbox {
    fn from(tx: Sender<Msg>) -> Self {
        Outbox { tx, tab: None }
    }
}

/// `msg` for `tab`, if any.
pub fn tag(tab: Option<TabId>, msg: Msg) -> Msg {
    match tab {
        Some(t) => Msg::Tab(t, Box::new(msg)),
        None => msg,
    }
}

/// Typed into a password prompt: kept out of debug output.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(pub String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(…)")
    }
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
        opts: DiffOpts,
    },
    LoadFile {
        seq: u64,
        index: usize,
        spec: DiffSpec,
        file: FileChange,
        opts: DiffOpts,
    },
    /// Write one setting to the config file (`None` removes it).
    SaveSetting {
        key: &'static str,
        value: Option<Value>,
    },
    /// Settings the worker uses (collapse rules, trunk depth) changed.
    SetWorkerConfig(Box<Config>),
    SetSyntaxTheme(two_face::theme::EmbeddedThemeName),
    /// Open the config file in the editor, then reload it.
    EditConfig,
    /// Re-read the settings: another repository's tab saved them.
    ReloadConfig,
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
    /// Commit exactly these files (`git.actions`).
    Commit {
        files: Vec<CommitFile>,
        message: String,
    },
    /// Find out what a push would do.
    PreparePush,
    Push {
        remote: String,
        branch: String,
        dest: String,
        set_upstream: bool,
    },
    /// Edit the commit message in git's editor.
    EditMessage(String),
    /// Reply to a password prompt; `None` cancels it.
    Answer {
        id: u64,
        answer: Option<Secret>,
    },
    Quit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn an_outbox_tags_its_messages() {
        let (tx, rx) = mpsc::channel();
        Outbox::tagged(tx.clone(), TabId(3))
            .send(Msg::Tick)
            .unwrap();
        Outbox::from(tx).send(Msg::Tick).unwrap();
        assert!(matches!(rx.recv(), Ok(Msg::Tab(TabId(3), m)) if matches!(*m, Msg::Tick)));
        assert!(matches!(rx.recv(), Ok(Msg::Tick)));
    }
}

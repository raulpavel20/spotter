//! Gitignore-aware file watching (PLAN §6.2).
//!
//! The worktree gets one non-recursive watch per non-ignored directory, so
//! `node_modules/` or `target/` never exhaust inotify watches. Inside the
//! git dirs only HEAD, the index, refs and in-progress state matter.

use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use ignore::WalkBuilder;
use notify::event::{AccessKind, AccessMode, CreateKind, EventKind};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

use crate::git::Repo;
use crate::msg::{Msg, RefreshKind, WatchStatus};

pub const DEBOUNCE: Duration = Duration::from_millis(250);

/// Which refresh a changed path calls for.
#[derive(Debug, Clone)]
pub struct Classifier {
    pub root: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
}

const REFS_FILES: &[&str] = &[
    "HEAD",
    "packed-refs",
    "MERGE_HEAD",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "BISECT_LOG",
];
const REFS_DIRS: &[&str] = &["refs", "reftable", "rebase-merge", "rebase-apply"];

impl Classifier {
    pub fn new(repo: &Repo) -> Self {
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_owned());
        Classifier {
            root: canon(&repo.root),
            git_dir: canon(&repo.git_dir),
            common_dir: canon(&repo.common_dir),
        }
    }

    pub fn classify(&self, path: &Path) -> Option<RefreshKind> {
        if path.extension().is_some_and(|e| e == "lock") {
            return None;
        }
        for dir in [&self.git_dir, &self.common_dir] {
            if let Ok(rel) = path.strip_prefix(dir) {
                let first = match rel.components().next() {
                    Some(Component::Normal(c)) => c.to_string_lossy().into_owned(),
                    // The git dir itself (e.g. a rename inside it).
                    _ => return None,
                };
                return if REFS_FILES.contains(&first.as_str())
                    || REFS_DIRS.contains(&first.as_str())
                {
                    Some(RefreshKind::Full)
                } else if first == "index" && rel.components().count() == 1 {
                    Some(RefreshKind::Worktree)
                } else {
                    // objects/, logs/, spotter/, COMMIT_EDITMSG, FETCH_HEAD…
                    None
                };
            }
        }
        let rel = path.strip_prefix(&self.root).ok()?;
        if rel.components().next() == Some(Component::Normal(".git".as_ref())) {
            return None;
        }
        Some(RefreshKind::Worktree)
    }
}

/// Events that mean something was written. Opens and reads are ignored,
/// otherwise Spotter's own git calls would trigger refreshes.
fn is_write(kind: &EventKind) -> bool {
    match kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) => false,
        _ => true,
    }
}

/// Non-ignored directories under `dir` (including `dir`).
fn walk_dirs(dir: &Path) -> Vec<PathBuf> {
    WalkBuilder::new(dir)
        .hidden(false)
        .parents(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(|e| e.file_name() != ".git")
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_dir()))
        .map(|e| e.into_path())
        .collect()
}

/// Whether a newly created directory is ignored, judged from its parent.
fn is_ignored_dir(dir: &Path) -> bool {
    let Some(parent) = dir.parent() else {
        return true;
    };
    let visible = WalkBuilder::new(parent)
        .hidden(false)
        .parents(true)
        .max_depth(Some(1))
        .filter_entry(|e| e.file_name() != ".git")
        .build()
        .filter_map(Result::ok)
        .any(|e| e.path() == dir);
    !visible
}

pub struct WatchHandle {
    _thread: thread::JoinHandle<()>,
}

/// Starts watching. On error the caller falls back to polling.
pub fn spawn(repo: &Repo, out: Sender<Msg>) -> Result<WatchHandle, String> {
    let cls = Classifier::new(repo);
    let (ev_tx, ev_rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |res| {
        let _ = ev_tx.send(res);
    })
    .map_err(|e| e.to_string())?;

    for dir in walk_dirs(&cls.root) {
        // A directory that vanished mid-walk is fine; anything else is not.
        if let Err(e) = watcher.watch(&dir, RecursiveMode::NonRecursive) {
            if dir.exists() {
                return Err(format!("{}: {e}", dir.display()));
            }
        }
    }
    let mut git_dirs = vec![cls.git_dir.clone()];
    if cls.common_dir != cls.git_dir {
        git_dirs.push(cls.common_dir.clone());
    }
    for d in &git_dirs {
        watcher
            .watch(d, RecursiveMode::NonRecursive)
            .map_err(|e| e.to_string())?;
        for sub in REFS_DIRS {
            let p = d.join(sub);
            if p.is_dir() {
                let _ = watcher.watch(&p, RecursiveMode::Recursive);
            }
        }
    }

    let thread = thread::Builder::new()
        .name("spotter-watch".into())
        .spawn(move || {
            let mut watcher = watcher;
            let handle = |ev: notify::Result<Event>,
                          watcher: &mut RecommendedWatcher|
             -> Option<RefreshKind> {
                let ev = match ev {
                    Ok(ev) => ev,
                    Err(_) => return Some(RefreshKind::Full),
                };
                if ev.need_rescan() {
                    return Some(RefreshKind::Full);
                }
                if !is_write(&ev.kind) {
                    return None;
                }
                let mut kind = None;
                for p in &ev.paths {
                    let k = cls.classify(p);
                    // New directories: watch them too (rebase-merge/, or
                    // non-ignored worktree dirs).
                    let created_dir = matches!(ev.kind, EventKind::Create(CreateKind::Folder))
                        || (matches!(ev.kind, EventKind::Create(_) | EventKind::Modify(_))
                            && p.is_dir());
                    if created_dir && p.is_dir() {
                        if p.starts_with(&cls.git_dir) || p.starts_with(&cls.common_dir) {
                            if k.is_some() {
                                let _ = watcher.watch(p, RecursiveMode::Recursive);
                            }
                        } else if k.is_some() && !is_ignored_dir(p) {
                            for d in walk_dirs(p) {
                                let _ = watcher.watch(&d, RecursiveMode::NonRecursive);
                            }
                        }
                    }
                    kind = kind.max(k);
                }
                kind
            };
            while let Ok(first) = ev_rx.recv() {
                let Some(mut kind) = handle(first, &mut watcher) else {
                    continue;
                };
                // Collect for a short window, then send one refresh.
                let deadline = Instant::now() + DEBOUNCE;
                loop {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        break;
                    }
                    match ev_rx.recv_timeout(left) {
                        Ok(ev) => kind = kind.max(handle(ev, &mut watcher).unwrap_or(kind)),
                        Err(mpsc::RecvTimeoutError::Timeout) => break,
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
                if out.send(Msg::Fs(kind)).is_err() {
                    return;
                }
            }
            let _ = out.send(Msg::Watch(WatchStatus::Error("watcher stopped".into())));
        })
        .map_err(|e| e.to_string())?;
    Ok(WatchHandle { _thread: thread })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cls() -> Classifier {
        Classifier {
            root: "/r".into(),
            git_dir: "/r/.git".into(),
            common_dir: "/r/.git".into(),
        }
    }

    #[test]
    fn classifies_paths() {
        let c = cls();
        let k = |p: &str| c.classify(Path::new(p));
        assert_eq!(k("/r/.git/HEAD"), Some(RefreshKind::Full));
        assert_eq!(k("/r/.git/refs/heads/main"), Some(RefreshKind::Full));
        assert_eq!(k("/r/.git/packed-refs"), Some(RefreshKind::Full));
        assert_eq!(k("/r/.git/rebase-merge/msgnum"), Some(RefreshKind::Full));
        assert_eq!(k("/r/.git/index"), Some(RefreshKind::Worktree));
        assert_eq!(k("/r/.git/index.lock"), None);
        assert_eq!(k("/r/.git/refs/heads/main.lock"), None);
        assert_eq!(k("/r/.git/objects/ab/cdef"), None);
        assert_eq!(k("/r/.git/spotter/viewed.json"), None);
        assert_eq!(k("/r/.git/COMMIT_EDITMSG"), None);
        assert_eq!(k("/r/src/main.rs"), Some(RefreshKind::Worktree));
        assert_eq!(k("/elsewhere/x"), None);
    }

    #[test]
    fn linked_worktree_dirs() {
        let c = Classifier {
            root: "/wt".into(),
            git_dir: "/r/.git/worktrees/wt".into(),
            common_dir: "/r/.git".into(),
        };
        assert_eq!(
            c.classify(Path::new("/r/.git/worktrees/wt/HEAD")),
            Some(RefreshKind::Full)
        );
        assert_eq!(
            c.classify(Path::new("/r/.git/worktrees/wt/index")),
            Some(RefreshKind::Worktree)
        );
        assert_eq!(
            c.classify(Path::new("/r/.git/refs/heads/x")),
            Some(RefreshKind::Full)
        );
        // Another worktree's index is not ours.
        assert_eq!(c.classify(Path::new("/r/.git/worktrees/other/index")), None);
        assert_eq!(c.classify(Path::new("/wt/.git")), None);
    }

    #[test]
    fn ignores_reads() {
        assert!(!is_write(&EventKind::Access(AccessKind::Open(
            AccessMode::Any
        ))));
        assert!(!is_write(&EventKind::Access(AccessKind::Close(
            AccessMode::Read
        ))));
        assert!(is_write(&EventKind::Access(AccessKind::Close(
            AccessMode::Write
        ))));
        assert!(is_write(&EventKind::Create(CreateKind::File)));
    }
}

//! The background worker: refreshes and patch loads off the UI thread,
//! with request coalescing and a patch cache.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use crate::config::Config;
use crate::git::Repo;
use crate::git::diff::{DiffOpts, DiffSpec};
use crate::model::FileChange;
use crate::msg::{Msg, Outbox, RefreshKind};
use crate::refresh::{self, Cache, LoadedPatch, RefreshOpts, Snapshot};

#[derive(Debug)]
pub enum Request {
    Refresh {
        seq: u64,
        kind: RefreshKind,
        opts: RefreshOpts,
    },
    Patch {
        seq: u64,
        spec: DiffSpec,
        files: Vec<FileChange>,
        opts: DiffOpts,
    },
    File {
        seq: u64,
        index: usize,
        spec: DiffSpec,
        file: FileChange,
        opts: DiffOpts,
    },
    /// New settings (collapse rules, trunk depth). A refresh follows.
    SetConfig(Config),
}

/// What one round of the worker does after draining its queue.
#[derive(Debug, Default)]
pub struct Batch {
    pub refresh: Option<(u64, RefreshKind, RefreshOpts)>,
    pub patch: Option<(u64, DiffSpec, Vec<FileChange>, DiffOpts)>,
    pub files: Vec<(u64, usize, DiffSpec, FileChange, DiffOpts)>,
    pub config: Option<Config>,
}

/// Coalesces queued requests: one refresh (the strongest kind, the latest
/// options), the latest patch, and every file load.
pub fn coalesce(reqs: impl IntoIterator<Item = Request>) -> Batch {
    let mut b = Batch::default();
    for r in reqs {
        match r {
            Request::Refresh { seq, kind, opts } => {
                b.refresh = Some(match b.refresh.take() {
                    Some((s, k, o)) => {
                        let (seq2, opts2) = if seq >= s { (seq, opts) } else { (s, o) };
                        (seq2, k.max(kind), opts2)
                    }
                    None => (seq, kind, opts),
                });
            }
            Request::Patch {
                seq,
                spec,
                files,
                opts,
            } => {
                if b.patch.as_ref().is_none_or(|(s, ..)| seq >= *s) {
                    b.patch = Some((seq, spec, files, opts));
                }
            }
            Request::File {
                seq,
                index,
                spec,
                file,
                opts,
            } => {
                if !b.files.iter().any(|(s, i, ..)| *s == seq && *i == index) {
                    b.files.push((seq, index, spec, file, opts));
                }
            }
            Request::SetConfig(cfg) => b.config = Some(cfg),
        }
    }
    b
}

/// Byte-budgeted LRU of immutable (commit) patches.
struct PatchCache {
    map: HashMap<(DiffSpec, DiffOpts), (LoadedPatch, usize)>,
    order: VecDeque<(DiffSpec, DiffOpts)>,
    bytes: usize,
    budget: usize,
}

fn patch_bytes(p: &LoadedPatch) -> usize {
    p.patches
        .iter()
        .flatten()
        .flat_map(|fp| fp.hunks.iter())
        .map(|h| h.lines.iter().map(|l| l.text.len() + 48).sum::<usize>())
        .sum::<usize>()
        + 256
}

impl PatchCache {
    fn new(budget: usize) -> Self {
        PatchCache {
            map: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            budget,
        }
    }

    fn get(&mut self, k: &(DiffSpec, DiffOpts), files: &[FileChange]) -> Option<LoadedPatch> {
        let (p, _) = self.map.get(k)?;
        if p.files != files {
            return None;
        }
        let p = p.clone();
        self.order.retain(|x| x != k);
        self.order.push_back(k.clone());
        Some(p)
    }

    fn put(&mut self, k: (DiffSpec, DiffOpts), p: LoadedPatch) {
        let size = patch_bytes(&p);
        if size > self.budget {
            return;
        }
        if let Some((_, old)) = self.map.insert(k.clone(), (p, size)) {
            self.bytes -= old;
            self.order.retain(|x| x != &k);
        }
        self.bytes += size;
        self.order.push_back(k);
        while self.bytes > self.budget {
            let Some(old) = self.order.pop_front() else {
                break;
            };
            if let Some((_, s)) = self.map.remove(&old) {
                self.bytes -= s;
            }
        }
    }
}

/// Patch cache size for one repository.
pub const PATCH_BUDGET: usize = 64 * 1024 * 1024;

pub struct Worker {
    repo: Repo,
    cfg: Config,
    base: Option<String>,
    cache: Cache,
    patches: PatchCache,
    last: Option<Snapshot>,
}

impl Worker {
    pub fn new(repo: Repo, cfg: Config, base: Option<String>) -> Self {
        Worker {
            repo,
            cfg,
            base,
            cache: Cache::default(),
            patches: PatchCache::new(PATCH_BUDGET),
            last: None,
        }
    }

    /// Caps the patch cache (several repositories share the memory).
    pub fn with_patch_budget(mut self, bytes: usize) -> Self {
        self.patches = PatchCache::new(bytes);
        self
    }

    pub fn refresh(&mut self, kind: RefreshKind, opts: &RefreshOpts) -> Result<Snapshot, String> {
        let base = self.base.as_deref();
        let result = match (kind, &self.last) {
            (RefreshKind::Worktree, Some(prev)) => {
                refresh::worktree(&self.repo, &self.cfg, base, opts, prev, &mut self.cache)
            }
            _ => refresh::full(
                &self.repo,
                &self.cfg,
                base,
                opts,
                self.last.as_ref(),
                &mut self.cache,
            ),
        };
        let snap = result.map_err(|e| {
            crate::log::line(|| format!("refresh {kind:?} failed: {e}"));
            e.to_string()
        })?;
        self.last = Some(snap.clone());
        Ok(snap)
    }

    pub fn patch(
        &mut self,
        spec: &DiffSpec,
        files: &[FileChange],
        opts: DiffOpts,
    ) -> Result<LoadedPatch, String> {
        let key = (spec.clone(), opts);
        if !spec.is_volatile()
            && let Some(p) = self.patches.get(&key, files)
        {
            return Ok(p);
        }
        let p = refresh::load_patch(&self.repo, spec, files, opts).map_err(|e| e.to_string())?;
        if !spec.is_volatile() && !p.lazy {
            self.patches.put(key, p.clone());
        }
        Ok(p)
    }

    /// New settings; merge file lists depend on nothing configurable, so
    /// only collapse flags and the base change (on the next refresh).
    pub fn set_config(&mut self, cfg: Config) {
        self.cfg = cfg;
    }

    fn run(mut self, rx: Receiver<Request>, out: Outbox) {
        while let Ok(first) = rx.recv() {
            let mut reqs = vec![first];
            reqs.extend(rx.try_iter());
            let batch = coalesce(reqs);
            if let Some(cfg) = batch.config {
                self.set_config(cfg);
            }
            if let Some((seq, kind, opts)) = batch.refresh {
                let result = self.refresh(kind, &opts).map(Box::new);
                if out.send(Msg::Refreshed { seq, result }).is_err() {
                    return;
                }
            }
            if let Some((seq, spec, files, opts)) = batch.patch {
                let result = self.patch(&spec, &files, opts);
                if out.send(Msg::PatchLoaded { seq, result }).is_err() {
                    return;
                }
            }
            for (seq, index, spec, file, opts) in batch.files {
                let result =
                    refresh::load_file(&self.repo, &spec, &file, opts).map_err(|e| e.to_string());
                if out.send(Msg::FileLoaded { seq, index, result }).is_err() {
                    return;
                }
            }
        }
    }
}

/// Starts the worker thread; drop the returned sender to stop it.
pub fn spawn(worker: Worker, out: impl Into<Outbox>) -> Sender<Request> {
    let out = out.into();
    let (tx, rx) = mpsc::channel();
    let name = match out.tab() {
        Some(t) => format!("spotter-worker-{}", t.0),
        None => "spotter-worker".into(),
    };
    thread::Builder::new()
        .name(name)
        .spawn(move || worker.run(rx, out))
        .expect("spawn worker thread");
    tx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesces_refreshes_and_keeps_latest_patch() {
        let spec = |s: &str| DiffSpec::Worktree(s.into());
        let b = coalesce([
            Request::Refresh {
                seq: 1,
                kind: RefreshKind::Worktree,
                opts: RefreshOpts::default(),
            },
            Request::Patch {
                seq: 1,
                spec: spec("a"),
                files: vec![],
                opts: DiffOpts::default(),
            },
            Request::Refresh {
                seq: 2,
                kind: RefreshKind::Full,
                opts: RefreshOpts::default(),
            },
            Request::Refresh {
                seq: 3,
                kind: RefreshKind::Worktree,
                opts: RefreshOpts {
                    include_wt: true,
                    ..Default::default()
                },
            },
            Request::Patch {
                seq: 2,
                spec: spec("b"),
                files: vec![],
                opts: DiffOpts::default(),
            },
        ]);
        let (seq, kind, opts) = b.refresh.unwrap();
        assert_eq!((seq, kind), (3, RefreshKind::Full));
        assert!(opts.include_wt);
        assert_eq!(b.patch.unwrap().1, spec("b"));
    }
}

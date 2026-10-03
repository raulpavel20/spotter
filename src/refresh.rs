//! Building snapshots and patches from git. Synchronous; the worker thread
//! runs these off the UI thread.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::Path;

use bstr::{BString, ByteSlice};
use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::config::Config;
use crate::git::base::{self, BaseInfo, BaseOptions};
use crate::git::diff::{self, DiffOpts, DiffSpec};
use crate::git::log;
use crate::git::patch::{self, bytes_to_os};
use crate::git::status;
use crate::git::{GitError, Repo, RepoOp, Sub};
use crate::model::{
    Collapse, Commit, FileChange, FilePatch, MODE_GITLINK, MODE_SYMLINK, Status, TargetId,
};

/// Above this many changed lines, a target's files load on demand.
pub const LAZY_LINES: u64 = 20_000;
/// Untracked files above this size get a summary only.
pub const MAX_UNTRACKED: u64 = 1024 * 1024;
/// Untracked files above this size aren't even read to count lines.
const MAX_COUNT: u64 = 64 * 1024 * 1024;

const BUILTIN_COLLAPSE: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "poetry.lock",
    "uv.lock",
    "Gemfile.lock",
    "composer.lock",
    "go.sum",
    "*.min.js",
    "*.min.css",
    "*.map",
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefreshOpts {
    /// Σ includes uncommitted changes.
    pub include_wt: bool,
    /// Merge commits shown against their first parent instead of remerge-diff.
    pub first_parent: HashSet<String>,
}

/// How HEAD moved since the previous snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadChange {
    Added(usize),
    Rewritten,
    Switched(Option<String>),
}

/// Everything the UI shows, as of one refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub base: BaseInfo,
    pub ops: Vec<RepoOp>,
    /// Newest first.
    pub commits: Vec<Commit>,
    pub uncommitted: Vec<FileChange>,
    /// `None` when there is no Σ (unborn repo).
    pub total: Option<Vec<FileChange>>,
    pub opts: RefreshOpts,
    pub remerge: bool,
    pub change: Option<HeadChange>,
}

impl Snapshot {
    pub fn files(&self, id: &TargetId) -> &[FileChange] {
        match id {
            TargetId::Uncommitted => &self.uncommitted,
            TargetId::Total => self.total.as_deref().unwrap_or_default(),
            TargetId::Commit(sha) => self
                .commits
                .iter()
                .find(|c| &c.sha == sha)
                .map(|c| c.files.as_slice())
                .unwrap_or_default(),
        }
    }

    pub fn commit(&self, sha: &str) -> Option<&Commit> {
        self.commits.iter().find(|c| c.sha == sha)
    }

    /// What a target's diff compares.
    pub fn spec(&self, id: &TargetId, empty_tree: &str) -> Option<DiffSpec> {
        let head = self.base.head.clone();
        match id {
            TargetId::Uncommitted => Some(DiffSpec::Worktree(
                head.unwrap_or_else(|| empty_tree.to_owned()),
            )),
            TargetId::Total => {
                let mb = self.base.merge_base.clone()?;
                if self.opts.include_wt {
                    Some(DiffSpec::Worktree(mb))
                } else {
                    Some(DiffSpec::Trees(mb, head?))
                }
            }
            TargetId::Commit(sha) => {
                let c = self.commit(sha)?;
                Some(commit_spec(
                    c,
                    self.remerge,
                    &self.opts.first_parent,
                    empty_tree,
                ))
            }
        }
    }
}

fn commit_spec(c: &Commit, remerge: bool, first_parent: &HashSet<String>, empty: &str) -> DiffSpec {
    if c.is_merge() && remerge && !first_parent.contains(&c.sha) {
        DiffSpec::Remerge(c.sha.clone())
    } else {
        let parent = c
            .parents
            .first()
            .cloned()
            .unwrap_or_else(|| empty.to_owned());
        DiffSpec::Trees(parent, c.sha.clone())
    }
}

/// File lists of merge commits, which `git log` doesn't print.
#[derive(Default)]
pub struct Cache {
    merges: HashMap<DiffSpec, Vec<FileChange>>,
}

/// A full refresh: base, timeline, working tree.
pub fn full(
    repo: &Repo,
    cfg: &Config,
    explicit_base: Option<&str>,
    opts: &RefreshOpts,
    prev: Option<&Snapshot>,
    cache: &mut Cache,
) -> Result<Snapshot, GitError> {
    let git = &repo.git;
    let explicit = explicit_base.or(cfg.base.as_deref());
    let base = base::resolve(
        git,
        &BaseOptions {
            explicit,
            trunk_depth: cfg.trunk_depth,
            empty_tree: &repo.empty_tree,
        },
    )?;
    let remerge = repo.supports_remerge_diff();
    let mut commits = log::timeline(git, &base.range, base::SANITY_LIMIT)?;
    for c in commits.iter_mut().filter(|c| c.is_merge()) {
        let spec = commit_spec(c, remerge, &opts.first_parent, &repo.empty_tree);
        if let Some(files) = cache.merges.get(&spec) {
            c.files = files.clone();
        } else {
            c.files = diff::file_list(git, &spec)?;
            cache.merges.insert(spec, c.files.clone());
        }
    }
    let mut snap = Snapshot {
        base,
        ops: repo.in_progress(),
        commits,
        uncommitted: Vec::new(),
        total: None,
        opts: opts.clone(),
        remerge,
        change: None,
    };
    let collapse = Collapser::new(repo, cfg);
    let mut all: Vec<&mut FileChange> = snap
        .commits
        .iter_mut()
        .flat_map(|c| c.files.iter_mut())
        .collect();
    collapse.apply(repo, &mut all);
    worktree_parts(repo, &collapse, &mut snap)?;
    snap.change = head_change(repo, prev, &snap)?;
    Ok(snap)
}

/// A worktree refresh: only ◌ and Σ (and in-progress state) change.
/// Falls back to a full refresh if HEAD moved.
pub fn worktree(
    repo: &Repo,
    cfg: &Config,
    explicit_base: Option<&str>,
    opts: &RefreshOpts,
    prev: &Snapshot,
    cache: &mut Cache,
) -> Result<Snapshot, GitError> {
    let head = base::verify(&repo.git, "HEAD")?;
    if head != prev.base.head || opts.first_parent != prev.opts.first_parent {
        return full(repo, cfg, explicit_base, opts, Some(prev), cache);
    }
    let mut snap = prev.clone();
    snap.opts = opts.clone();
    snap.ops = repo.in_progress();
    snap.change = None;
    let collapse = Collapser::new(repo, cfg);
    worktree_parts(repo, &collapse, &mut snap)?;
    Ok(snap)
}

fn head_change(
    repo: &Repo,
    prev: Option<&Snapshot>,
    snap: &Snapshot,
) -> Result<Option<HeadChange>, GitError> {
    let Some(prev) = prev else { return Ok(None) };
    if prev.base.branch != snap.base.branch {
        return Ok(Some(HeadChange::Switched(snap.base.branch.clone())));
    }
    let (Some(old), Some(new)) = (&prev.base.head, &snap.base.head) else {
        return Ok(None);
    };
    if old == new {
        return Ok(None);
    }
    let is_ancestor = repo
        .git
        .cmd(Sub::MergeBase)
        .args(["--is-ancestor", old, new])
        .ok_codes(&[0, 1, 128])
        .run()?
        .code
        == 0;
    if is_ancestor {
        Ok(Some(HeadChange::Added(base::count(
            &repo.git,
            &format!("{old}..{new}"),
        )?)))
    } else {
        Ok(Some(HeadChange::Rewritten))
    }
}

fn worktree_parts(repo: &Repo, collapse: &Collapser, snap: &mut Snapshot) -> Result<(), GitError> {
    let git = &repo.git;
    let head = snap
        .base
        .head
        .clone()
        .unwrap_or_else(|| repo.empty_tree.clone());
    let st = status::status(git)?;
    let mut unc = diff::file_list(git, &DiffSpec::Worktree(head))?;
    mark_unmerged(&mut unc, &st.unmerged, repo);
    let untracked = untracked_files(repo, &st.untracked);
    unc.extend(untracked.iter().cloned());

    let mut total = match (&snap.base.merge_base, &snap.base.head) {
        (Some(mb), Some(head)) => {
            if snap.opts.include_wt {
                let mut t = diff::file_list(git, &DiffSpec::Worktree(mb.clone()))?;
                mark_unmerged(&mut t, &st.unmerged, repo);
                t.extend(untracked);
                Some(t)
            } else {
                Some(diff::file_list(
                    git,
                    &DiffSpec::Trees(mb.clone(), head.clone()),
                )?)
            }
        }
        _ => None,
    };

    // Working-tree blob ids, shared between ◌ and Σ.
    let null = repo.null_oid();
    let mut want: Vec<BString> = Vec::new();
    let needs = |f: &FileChange| {
        f.new_oid == null && f.status != Status::Deleted && f.new_mode != MODE_GITLINK
    };
    for f in unc.iter().chain(total.iter().flatten()) {
        if needs(f) && !want.contains(&f.path) {
            want.push(f.path.clone());
        }
    }
    let hashes = hash_worktree(repo, &want)?;
    for f in unc.iter_mut().chain(total.iter_mut().flatten()) {
        if needs(f) {
            if let Some(h) = hashes.get(&f.path) {
                f.new_oid = h.clone();
            }
        }
    }

    // Without index refreshes (see `cmd::GLOBAL_ARGS`), stat-dirty files
    // with unchanged content are listed too; their hash gives them away.
    let changed = |f: &FileChange| {
        !(f.status == Status::Modified && f.old_oid == f.new_oid && f.old_mode == f.new_mode)
    };
    unc.retain(changed);
    if let Some(t) = &mut total {
        t.retain(changed);
    }

    let mut all: Vec<&mut FileChange> = unc.iter_mut().chain(total.iter_mut().flatten()).collect();
    collapse.apply(repo, &mut all);
    snap.uncommitted = unc;
    snap.total = total;
    Ok(())
}

fn mark_unmerged(files: &mut Vec<FileChange>, unmerged: &[BString], repo: &Repo) {
    let null = repo.null_oid();
    // `git diff <tree>` reports a conflicted path twice: once as `U`, once
    // as a modification. Keep one entry with status `U`.
    let mut seen = HashSet::new();
    files.retain(|f| f.status != Status::Unmerged || seen.insert(f.path.clone()));
    for path in unmerged {
        let mut found = false;
        for f in files.iter_mut().filter(|f| &f.path == path) {
            f.status = Status::Unmerged;
            found = true;
        }
        if !found {
            files.push(FileChange {
                path: path.clone(),
                old_path: None,
                status: Status::Unmerged,
                old_mode: 0o100644,
                new_mode: 0o100644,
                old_oid: null.clone(),
                new_oid: null.clone(),
                added: None,
                deleted: None,
                collapse: None,
                size: None,
            });
        }
    }
    let mut seen = HashSet::new();
    files.retain(|f| seen.insert(f.path.clone()));
}

/// Untracked entries with line counts, read straight from the file system.
fn untracked_files(repo: &Repo, paths: &[BString]) -> Vec<FileChange> {
    let null = repo.null_oid();
    paths
        .iter()
        .map(|p| {
            let mut f = FileChange {
                path: p.clone(),
                old_path: None,
                status: Status::Untracked,
                old_mode: 0,
                new_mode: 0o100644,
                old_oid: null.clone(),
                new_oid: null.clone(),
                added: Some(0),
                deleted: Some(0),
                collapse: None,
                size: None,
            };
            let full = repo.root.join(bytes_to_os(p.trim_end_with(|c| c == '/')));
            match fs::symlink_metadata(&full) {
                Ok(m) if m.file_type().is_symlink() => {
                    f.new_mode = MODE_SYMLINK;
                    f.added = Some(1);
                    f.size = Some(m.len());
                }
                Ok(m) if m.is_dir() => {
                    // A nested repository shows up as `dir/`.
                    f.new_mode = MODE_GITLINK;
                }
                Ok(m) => {
                    f.size = Some(m.len());
                    if is_executable(&m) {
                        f.new_mode = 0o100755;
                    }
                    match count_lines(&full, m.len()) {
                        Some(n) => f.added = Some(n),
                        None => {
                            f.added = None;
                            f.deleted = None;
                        }
                    }
                }
                Err(_) => {}
            }
            f
        })
        .collect()
}

#[cfg(unix)]
fn is_executable(m: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    m.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_: &fs::Metadata) -> bool {
    false
}

/// Git's binary heuristic: a NUL byte in the first 8000 bytes.
pub fn looks_binary(head: &[u8]) -> bool {
    head[..head.len().min(8000)].contains(&0)
}

/// Line count of a text file, `None` if it looks binary.
fn count_lines(path: &Path, size: u64) -> Option<u64> {
    let mut file = fs::File::open(path).ok()?;
    let mut head = vec![0; 8000];
    let n = read_full(&mut file, &mut head);
    head.truncate(n);
    if looks_binary(&head) {
        return None;
    }
    if size > MAX_COUNT {
        return Some(0);
    }
    let mut count = head.iter().filter(|&&b| b == b'\n').count() as u64;
    let mut last = head.last().copied();
    let mut buf = vec![0; 64 * 1024];
    loop {
        let n = read_full(&mut file, &mut buf);
        if n == 0 {
            break;
        }
        count += buf[..n].iter().filter(|&&b| b == b'\n').count() as u64;
        last = Some(buf[n - 1]);
    }
    if last.is_some_and(|b| b != b'\n') {
        count += 1;
    }
    Some(count)
}

fn read_full(r: &mut impl Read, buf: &mut [u8]) -> usize {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) | Err(_) => break,
            Ok(k) => n += k,
        }
    }
    n
}

/// Blob ids of working-tree files as git would store them (clean filters
/// applied), without writing any object.
pub fn hash_worktree(repo: &Repo, paths: &[BString]) -> Result<HashMap<BString, String>, GitError> {
    let git = &repo.git;
    let mut out = HashMap::new();
    let mut batch: Vec<&BString> = Vec::new();
    for p in paths {
        let full = repo.root.join(bytes_to_os(p));
        let Ok(meta) = fs::symlink_metadata(&full) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            // Git stores the link text, not the target's content.
            if let Ok(target) = fs::read_link(&full) {
                let bytes = target.into_os_string().into_encoded_bytes();
                let oid = git
                    .cmd(Sub::HashObject)
                    .args(["-t", "blob", "--stdin"])
                    .stdin(bytes)
                    .line()?;
                out.insert(p.clone(), oid);
            }
        } else if meta.is_file() {
            if p.contains(&b'\n') {
                let oid = git
                    .cmd(Sub::HashObject)
                    .arg("--")
                    .arg(bytes_to_os(p))
                    .line()?;
                out.insert(p.clone(), oid);
            } else {
                batch.push(p);
            }
        }
    }
    if !batch.is_empty() {
        let mut input = Vec::new();
        for p in &batch {
            input.extend_from_slice(p);
            input.push(b'\n');
        }
        let res = git
            .cmd(Sub::HashObject)
            .arg("--stdin-paths")
            .stdin(input)
            .out()?;
        for (p, oid) in batch.into_iter().zip(res.lines()) {
            out.insert(p.clone(), oid.to_str_lossy().into_owned());
        }
    }
    Ok(out)
}

/// Collapse rules (PLAN §6.4).
pub struct Collapser {
    patterns: Gitignore,
    /// Files with more changed lines start collapsed.
    lines: u64,
}

impl Collapser {
    pub fn new(repo: &Repo, cfg: &Config) -> Self {
        let mut b = GitignoreBuilder::new(&repo.root);
        for p in BUILTIN_COLLAPSE
            .iter()
            .copied()
            .chain(cfg.collapse.iter().map(String::as_str))
        {
            let _ = b.add_line(None, p);
        }
        Collapser {
            patterns: b.build().unwrap_or_else(|_| Gitignore::empty()),
            lines: cfg.collapse_lines,
        }
    }

    pub fn apply(&self, repo: &Repo, files: &mut [&mut FileChange]) {
        let mut paths: Vec<BString> = files.iter().map(|f| f.path.clone()).collect();
        paths.sort();
        paths.dedup();
        let attrs = check_attrs(repo, &paths).unwrap_or_default();
        for f in files.iter_mut() {
            let rel = Path::new(&bytes_to_os(&f.path)).to_owned();
            f.collapse = if self
                .patterns
                .matched_path_or_any_parents(&rel, false)
                .is_ignore()
            {
                Some(Collapse::Lockfile)
            } else if let Some(c) = attrs.get(&f.path) {
                Some(*c)
            } else if f.lines_changed() > self.lines {
                Some(Collapse::Large)
            } else {
                None
            };
        }
    }
}

/// `linguist-generated` and `-diff` from gitattributes.
fn check_attrs(repo: &Repo, paths: &[BString]) -> Result<HashMap<BString, Collapse>, GitError> {
    let mut out = HashMap::new();
    if paths.is_empty() {
        return Ok(out);
    }
    let mut input = Vec::new();
    for p in paths {
        input.extend_from_slice(p);
        input.push(0);
    }
    let res = repo
        .git
        .cmd(Sub::CheckAttr)
        .args(["-z", "--stdin", "linguist-generated", "diff"])
        .stdin(input)
        .out()?;
    let fields: Vec<&[u8]> = res.split_str("\0").collect();
    for rec in fields.chunks(3) {
        let [path, attr, value] = rec else { continue };
        let c = match (*attr, *value) {
            (b"linguist-generated", b"set" | b"true") => Some(Collapse::Generated),
            (b"diff", b"unset") => Some(Collapse::NoDiff),
            _ => None,
        };
        if let Some(c) = c {
            out.entry(BString::from(*path)).or_insert(c);
        }
    }
    Ok(out)
}

/// A loaded patch for a target: one slot per file in `files`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedPatch {
    pub files: Vec<FileChange>,
    pub patches: Vec<Option<FilePatch>>,
    /// Too big to load at once: files load as they are visited.
    pub lazy: bool,
}

/// Loads a whole target's patch, or nothing if it is too big (`lazy`).
pub fn load_patch(
    repo: &Repo,
    spec: &DiffSpec,
    files: &[FileChange],
    opts: DiffOpts,
) -> Result<LoadedPatch, GitError> {
    let lines: u64 = files.iter().map(FileChange::lines_changed).sum();
    if lines > LAZY_LINES {
        return Ok(LoadedPatch {
            files: files.to_vec(),
            patches: vec![None; files.len()],
            lazy: true,
        });
    }
    let tracked: Vec<FileChange> = files
        .iter()
        .filter(|f| f.status != Status::Untracked)
        .cloned()
        .collect();
    let mut patches = Vec::with_capacity(files.len());
    let mut attached = if tracked.is_empty() {
        Vec::new()
    } else {
        patch::attach(&tracked, patch::load(&repo.git, spec, None, opts)?)
    }
    .into_iter();
    for f in files {
        let mut p = if f.status == Status::Untracked {
            untracked_patch(repo, f)
        } else {
            attached.next().flatten().unwrap_or_else(|| empty_patch(f))
        };
        fill_sizes(repo, spec, f, &mut p);
        crate::worddiff::annotate(&mut p);
        patches.push(Some(p));
    }
    Ok(LoadedPatch {
        files: files.to_vec(),
        patches,
        lazy: false,
    })
}

/// Loads one file's patch (lazy targets, or collapsed files).
pub fn load_file(
    repo: &Repo,
    spec: &DiffSpec,
    f: &FileChange,
    opts: DiffOpts,
) -> Result<FilePatch, GitError> {
    let mut p = if f.status == Status::Untracked {
        untracked_patch(repo, f)
    } else {
        let mut paths: Vec<&[u8]> = vec![&f.path];
        if let Some(old) = &f.old_path {
            paths.push(old);
        }
        let parsed = patch::load(&repo.git, spec, Some(&paths), opts)?;
        patch::attach(std::slice::from_ref(f), parsed)
            .pop()
            .flatten()
            .unwrap_or_else(|| empty_patch(f))
    };
    fill_sizes(repo, spec, f, &mut p);
    crate::worddiff::annotate(&mut p);
    Ok(p)
}

/// A file with no textual changes (pure rename, mode change, or the
/// working tree changed under us).
fn empty_patch(f: &FileChange) -> FilePatch {
    FilePatch {
        header: patch::expected_header(f),
        ..FilePatch::default()
    }
}

fn untracked_patch(repo: &Repo, f: &FileChange) -> FilePatch {
    let mut p = FilePatch {
        header: patch::expected_header(f),
        meta: vec![BString::from(format!("new file mode {:o}", f.new_mode))],
        ..FilePatch::default()
    };
    if f.is_binary() || f.new_mode == MODE_GITLINK || f.size.unwrap_or(0) > MAX_UNTRACKED {
        p.binary = f.is_binary();
        return p;
    }
    let full = repo.root.join(bytes_to_os(&f.path));
    let content = if f.new_mode == MODE_SYMLINK {
        fs::read_link(&full).map(|t| t.into_os_string().into_encoded_bytes())
    } else {
        fs::read(&full)
    };
    match content {
        Ok(c) => patch::untracked_patch(f, &c),
        Err(_) => p,
    }
}

/// Size of a blob, for binary summaries.
pub fn blob_size(repo: &Repo, oid: &str) -> Option<u64> {
    if oid.bytes().all(|b| b == b'0') {
        return None;
    }
    repo.git
        .cmd(Sub::CatFile)
        .args(["-s", oid])
        .line()
        .ok()?
        .parse()
        .ok()
}

/// Fills in sizes for a binary file's summary line.
fn fill_sizes(repo: &Repo, spec: &DiffSpec, f: &FileChange, p: &mut FilePatch) {
    if !(p.binary || f.is_binary()) {
        return;
    }
    p.binary = true;
    if f.status != Status::Added && f.status != Status::Untracked {
        p.old_size = blob_size(repo, &f.old_oid);
    }
    if f.status != Status::Deleted {
        p.new_size = if spec.is_volatile() {
            fs::metadata(repo.root.join(bytes_to_os(&f.path)))
                .ok()
                .map(|m| m.len())
        } else {
            blob_size(repo, &f.new_oid)
        };
    }
}

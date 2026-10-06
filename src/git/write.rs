//! The git commands that write: committing the files you pick, and pushing
//! the current branch. They run only when you confirm them in a panel (with
//! `git.actions` on), never as part of a refresh, and only through the
//! [`WriteSub`] allowlist.

use std::ffi::OsString;

use bstr::BString;

use super::cmd::{GitError, Sub, WriteSub};
use super::patch::bytes_to_os;
use super::repo::Repo;

/// Extra environment for writes: the password helper.
pub type Env = [(OsString, OsString)];

pub const BUSY: &str =
    "git is busy: another git command (likely the agent's) holds the index lock; try again";

/// A file to commit, as it is on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitFile {
    pub path: BString,
    /// A rename's old path, committed as its deletion.
    pub old_path: Option<BString>,
    /// Not known to git yet, so it is added first.
    pub untracked: bool,
}

/// What git said when a write failed: its last lines, hook output
/// included.
fn failure(e: GitError) -> String {
    let lines: Vec<&str> = e
        .stderr
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return e.to_string();
    }
    lines[lines.len().saturating_sub(12)..].join("\n")
}

/// Commits exactly `files`, as they are on disk, with `message`. Anything
/// else in the index stays staged and out of the commit (`--only`).
/// Returns the new commit's short id.
pub fn commit(
    repo: &Repo,
    files: &[CommitFile],
    message: &str,
    env: &Env,
) -> Result<String, String> {
    if files.is_empty() {
        return Err("pick at least one file".into());
    }
    if repo.git_dir.join("index.lock").exists() {
        return Err(BUSY.into());
    }
    let untracked: Vec<OsString> = files
        .iter()
        .filter(|f| f.untracked)
        .map(|f| bytes_to_os(&f.path))
        .collect();
    // `commit --only` refuses paths git doesn't know yet.
    if !untracked.is_empty() {
        repo.git
            .write(WriteSub::Add)
            .arg("--")
            .args(&untracked)
            .envs(env)
            .run()
            .map_err(failure)?;
    }
    let mut paths = Vec::new();
    for f in files {
        paths.push(bytes_to_os(&f.path));
        if let Some(old) = &f.old_path {
            paths.push(bytes_to_os(old));
        }
    }
    let committed = repo
        .git
        .write(WriteSub::Commit)
        .args(["-F", "-", "--only", "--"])
        .args(&paths)
        .stdin(message.as_bytes().to_vec())
        .envs(env)
        .run();
    if let Err(e) = committed {
        // Leave new files untracked again, as they were.
        if !untracked.is_empty() {
            let _ = repo
                .git
                .write(WriteSub::Reset)
                .args(["-q", "--"])
                .args(&untracked)
                .run();
        }
        return Err(failure(e));
    }
    repo.git
        .cmd(Sub::RevParse)
        .args(["--short", "HEAD"])
        .line()
        .map_err(|e| e.to_string())
}

/// Whether commits are signed with gpg, whose pinentry needs the real
/// terminal (ssh signing asks through the password helper instead).
pub fn signs_with_gpg(repo: &Repo) -> bool {
    let get = |args: &[&str]| {
        repo.git
            .cmd(Sub::Config)
            .args(args)
            .ok_codes(&[0, 1])
            .line()
            .unwrap_or_default()
    };
    get(&["--type=bool", "--get", "commit.gpgsign"]) == "true"
        && get(&["--get", "gpg.format"]) != "ssh"
}

/// The editor git uses for commit messages (`git var GIT_EDITOR`).
pub fn editor(repo: &Repo) -> Result<String, String> {
    repo.git
        .cmd(Sub::Var)
        .arg("GIT_EDITOR")
        .line()
        .map_err(|e| e.to_string())
}

/// What `P` would push, found with read-only commands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PushInfo {
    /// The current branch; `None` when HEAD is detached.
    pub branch: Option<String>,
    /// Where to push: the upstream's remote, else `origin`, else the only
    /// remote.
    pub remote: Option<String>,
    /// The upstream as `remote/branch`, when the branch tracks one there.
    pub upstream: Option<String>,
    /// The remote branch to update, `refs/heads/…`.
    pub dest: String,
    /// Commits the push would send.
    pub ahead: usize,
}

pub fn push_info(repo: &Repo) -> Result<PushInfo, String> {
    let git = &repo.git;
    let get = |key: &str| {
        git.cmd(Sub::Config)
            .args(["--get", key])
            .ok_codes(&[0, 1])
            .line()
            .ok()
            .filter(|s| !s.is_empty())
    };
    let branch = git
        .cmd(Sub::SymbolicRef)
        .args(["--short", "-q", "HEAD"])
        .ok_codes(&[0, 1])
        .line()
        .map_err(|e| e.to_string())?;
    if branch.is_empty() {
        return Ok(PushInfo::default());
    }
    let urls = git
        .cmd(Sub::Config)
        .args(["--get-regexp", r"^remote\..*\.url$"])
        .ok_codes(&[0, 1])
        .line()
        .map_err(|e| e.to_string())?;
    let remotes: Vec<String> = urls
        .lines()
        .filter_map(|l| l.split_once(' '))
        .filter_map(|(k, _)| k.strip_prefix("remote.")?.strip_suffix(".url"))
        .map(str::to_owned)
        .collect();
    let up_remote = get(&format!("branch.{branch}.remote")).filter(|r| remotes.contains(r));
    let up_merge = get(&format!("branch.{branch}.merge"));
    let remote = up_remote
        .clone()
        .or_else(|| remotes.iter().find(|r| *r == "origin").cloned())
        .or_else(|| (remotes.len() == 1).then(|| remotes[0].clone()));
    let tracked = up_remote.is_some() && up_remote == remote;
    let (upstream, dest) = match (&remote, up_merge) {
        (Some(r), Some(merge)) if tracked => {
            let short = merge.strip_prefix("refs/heads/").unwrap_or(&merge);
            (Some(format!("{r}/{short}")), merge.clone())
        }
        _ => (None, format!("refs/heads/{branch}")),
    };
    let count = |range: &[&str]| {
        git.cmd(Sub::RevList)
            .arg("--count")
            .args(range)
            .line()
            .ok()
            .and_then(|n| n.parse::<usize>().ok())
    };
    // Commits the upstream lacks; without one, those on no remote at all.
    let ahead = upstream
        .as_ref()
        .and_then(|_| count(&["@{upstream}..HEAD"]))
        .or_else(|| count(&["HEAD", "--not", "--remotes"]))
        .unwrap_or(0);
    Ok(PushInfo {
        branch: Some(branch),
        remote,
        upstream,
        dest,
        ahead,
    })
}

/// Pushes `branch` to `dest` on `remote`, setting it as the upstream when
/// asked. Never forces. Returns what happened, for a toast.
pub fn push(
    repo: &Repo,
    remote: &str,
    branch: &str,
    dest: &str,
    set_upstream: bool,
    env: &Env,
) -> Result<String, String> {
    let spec = format!("refs/heads/{branch}:{dest}");
    let mut cmd = repo.git.write(WriteSub::Push).arg("--porcelain");
    if set_upstream {
        cmd = cmd.arg("-u");
    }
    let out = cmd.args([remote, &spec]).envs(env).run().map_err(|e| {
        let rejected = ["[rejected]", "Updates were rejected", "non-fast-forward"];
        if rejected.iter().any(|r| e.stderr.contains(r)) {
            "the remote has commits you don't have: pull in git, then push again".into()
        } else {
            failure(e)
        }
    })?;
    let short = dest.strip_prefix("refs/heads/").unwrap_or(dest);
    let flags = String::from_utf8_lossy(&out.stdout);
    // Porcelain lines: `<flag>\t<from>:<to>\t<summary>`.
    let flag = flags
        .lines()
        .find(|l| l.contains('\t'))
        .and_then(|l| l.chars().next());
    Ok(match flag {
        Some('=') => format!("{remote}/{short} is already up to date"),
        Some('*') => format!("published {branch} to {remote}"),
        _ => format!("pushed to {remote}/{short}"),
    })
}

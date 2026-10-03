//! Repository discovery, git version and in-progress operation detection.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use super::cmd::{Git, GitError, Sub};

pub const MIN_VERSION: GitVersion = GitVersion(2, 30, 0);
pub const REMERGE_VERSION: GitVersion = GitVersion(2, 36, 0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct GitVersion(pub u32, pub u32, pub u32);

impl fmt::Display for GitVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

impl GitVersion {
    /// Parses `git version 2.43.0`, `git version 2.39.3 (Apple Git-146)`,
    /// `git version 2.45.1.windows.1`.
    pub fn parse(s: &str) -> Option<GitVersion> {
        let rest = s.trim().strip_prefix("git version ")?;
        let mut nums = rest
            .split(|c: char| !c.is_ascii_digit())
            .filter(|p| !p.is_empty())
            .map(|p| p.parse::<u32>().ok());
        let major = nums.next()??;
        let minor = nums.next()??;
        let patch = nums.next().flatten().unwrap_or(0);
        Some(GitVersion(major, minor, patch))
    }
}

/// Why spotter cannot start in a directory.
#[derive(Debug)]
pub enum DiscoverError {
    NotARepo(PathBuf),
    Bare(PathBuf),
    GitTooOld(GitVersion),
    Git(GitError),
}

impl fmt::Display for DiscoverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DiscoverError::NotARepo(p) => {
                write!(f, "not a git repository: {}", p.display())
            }
            DiscoverError::Bare(p) => write!(
                f,
                "{} is a bare repository; spotter needs a working tree",
                p.display()
            ),
            DiscoverError::GitTooOld(v) => write!(
                f,
                "git {v} is too old; spotter needs git {MIN_VERSION} or newer"
            ),
            DiscoverError::Git(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DiscoverError {}

/// A discovered repository with a working tree.
#[derive(Debug, Clone)]
pub struct Repo {
    pub git: Git,
    /// Top level of the working tree.
    pub root: PathBuf,
    /// Per-worktree git dir (`.git`, or `.git/worktrees/<name>`).
    pub git_dir: PathBuf,
    /// Shared git dir; equals `git_dir` outside linked worktrees.
    pub common_dir: PathBuf,
    pub version: GitVersion,
    /// The empty tree id for this repo's object format.
    pub empty_tree: String,
}

impl Repo {
    pub fn discover(path: &Path) -> Result<Repo, DiscoverError> {
        Repo::discover_with_env(path, Vec::new())
    }

    pub fn discover_with_env(
        path: &Path,
        env: Vec<(OsString, OsString)>,
    ) -> Result<Repo, DiscoverError> {
        let path = fs::canonicalize(path).map_err(|_| DiscoverError::NotARepo(path.into()))?;
        let mut git = Git::new(&path).with_env(env);

        let version_line = git.cmd(Sub::Version).line().map_err(DiscoverError::Git)?;
        let version = GitVersion::parse(&version_line).unwrap_or(MIN_VERSION);
        if version < MIN_VERSION {
            return Err(DiscoverError::GitTooOld(version));
        }

        let bare = git
            .cmd(Sub::RevParse)
            .arg("--is-bare-repository")
            .line()
            .map_err(|e| {
                if e.stderr.contains("not a git repository") {
                    DiscoverError::NotARepo(path.clone())
                } else {
                    DiscoverError::Git(e)
                }
            })?;
        if bare == "true" {
            return Err(DiscoverError::Bare(path));
        }
        let inside = git
            .cmd(Sub::RevParse)
            .arg("--is-inside-work-tree")
            .line()
            .map_err(DiscoverError::Git)?;
        if inside != "true" {
            // e.g. run from inside the .git directory
            return Err(DiscoverError::NotARepo(path));
        }

        let out = git
            .cmd(Sub::RevParse)
            .args(["--show-toplevel", "--absolute-git-dir", "--git-common-dir"])
            .line()
            .map_err(DiscoverError::Git)?;
        let mut lines = out.lines();
        let (Some(top), Some(git_dir), Some(common)) = (lines.next(), lines.next(), lines.next())
        else {
            return Err(DiscoverError::NotARepo(path));
        };
        let root = PathBuf::from(top);
        let git_dir = PathBuf::from(git_dir);
        // --git-common-dir may be relative to the directory git ran in.
        let common = PathBuf::from(common);
        let common_dir = if common.is_absolute() {
            common
        } else {
            path.join(common)
        };
        let common_dir = fs::canonicalize(&common_dir).unwrap_or(common_dir);

        git.set_dir(&root);
        let empty_tree = git
            .cmd(Sub::HashObject)
            .args(["-t", "tree", "--stdin"])
            .stdin(Vec::new())
            .line()
            .map_err(DiscoverError::Git)?;

        Ok(Repo {
            git,
            root,
            git_dir,
            common_dir,
            version,
            empty_tree,
        })
    }

    pub fn supports_remerge_diff(&self) -> bool {
        self.version >= REMERGE_VERSION
    }

    /// All-zero object id of this repo's hash length.
    pub fn null_oid(&self) -> String {
        "0".repeat(self.empty_tree.len())
    }

    /// Where viewed marks live.
    pub fn marks_path(&self) -> PathBuf {
        self.common_dir.join("spotter").join("viewed.json")
    }

    /// In-progress operations, read straight from the git dir.
    pub fn in_progress(&self) -> Vec<RepoOp> {
        in_progress(&self.git_dir)
    }
}

/// An operation git is in the middle of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoOp {
    Rebase { step: Option<(u32, u32)> },
    Am,
    Merge,
    CherryPick,
    Revert,
    Bisect,
}

impl fmt::Display for RepoOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RepoOp::Rebase { step: Some((n, m)) } => write!(f, "rebase in progress ({n}/{m})"),
            RepoOp::Rebase { step: None } => write!(f, "rebase in progress"),
            RepoOp::Am => write!(f, "am in progress"),
            RepoOp::Merge => write!(f, "merge in progress"),
            RepoOp::CherryPick => write!(f, "cherry-pick in progress"),
            RepoOp::Revert => write!(f, "revert in progress"),
            RepoOp::Bisect => write!(f, "bisect in progress"),
        }
    }
}

fn read_num(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

pub fn in_progress(git_dir: &Path) -> Vec<RepoOp> {
    let mut ops = Vec::new();
    let merge_dir = git_dir.join("rebase-merge");
    let apply_dir = git_dir.join("rebase-apply");
    if merge_dir.is_dir() {
        let step = read_num(&merge_dir.join("msgnum")).zip(read_num(&merge_dir.join("end")));
        ops.push(RepoOp::Rebase { step });
    } else if apply_dir.is_dir() {
        if apply_dir.join("applying").exists() {
            ops.push(RepoOp::Am);
        } else {
            let step = read_num(&apply_dir.join("next")).zip(read_num(&apply_dir.join("last")));
            ops.push(RepoOp::Rebase { step });
        }
    }
    if git_dir.join("MERGE_HEAD").exists() {
        ops.push(RepoOp::Merge);
    }
    if git_dir.join("CHERRY_PICK_HEAD").exists() {
        ops.push(RepoOp::CherryPick);
    }
    if git_dir.join("REVERT_HEAD").exists() {
        ops.push(RepoOp::Revert);
    }
    if git_dir.join("BISECT_LOG").exists() {
        ops.push(RepoOp::Bisect);
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions() {
        assert_eq!(
            GitVersion::parse("git version 2.43.0"),
            Some(GitVersion(2, 43, 0))
        );
        assert_eq!(
            GitVersion::parse("git version 2.39.3 (Apple Git-146)"),
            Some(GitVersion(2, 39, 3))
        );
        assert_eq!(
            GitVersion::parse("git version 2.45.1.windows.1\n"),
            Some(GitVersion(2, 45, 1))
        );
        assert_eq!(
            GitVersion::parse("git version 3.0"),
            Some(GitVersion(3, 0, 0))
        );
        assert_eq!(GitVersion::parse("nope"), None);
        assert!(GitVersion(2, 34, 1) < REMERGE_VERSION);
    }
}

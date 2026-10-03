//! Base branch, merge-base and trunk-mode resolution (PLAN §6.1).

use bstr::ByteSlice;

use super::cmd::{Git, GitError, Sub};
use super::log::LogRange;

/// More commits than this between base and HEAD suggests a wrong base.
pub const SANITY_LIMIT: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseMode {
    /// A feature branch or detached HEAD compared against a base branch.
    Feature,
    /// `--base` or `spotter.base`.
    Explicit,
    /// On the default branch, compared against its upstream.
    TrunkUpstream,
    /// On the default branch with no upstream: the last N commits.
    TrunkRecent,
    /// No usable base found: the last N commits.
    NoBase,
    /// No commits yet.
    Unborn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseInfo {
    /// HEAD commit; `None` in an unborn repo.
    pub head: Option<String>,
    /// Current branch short name; `None` when detached.
    pub branch: Option<String>,
    pub mode: BaseMode,
    /// Display name of the base ref.
    pub base_ref: Option<String>,
    /// Where Σ starts: the merge-base, or the commit (or empty tree) below
    /// the oldest shown commit.
    pub merge_base: Option<String>,
    pub range: LogRange,
    /// Commits in the range before truncation.
    pub count: usize,
    /// Banner lines.
    pub notes: Vec<String>,
    /// Shown in place of an empty timeline.
    pub hint: Option<String>,
}

impl BaseInfo {
    pub fn truncated(&self) -> bool {
        self.count > SANITY_LIMIT
    }
}

pub struct BaseOptions<'a> {
    pub explicit: Option<&'a str>,
    pub trunk_depth: usize,
    pub empty_tree: &'a str,
}

/// `rev-parse --verify` a commit-ish; `None` if it doesn't resolve.
pub fn verify(git: &Git, rev: &str) -> Result<Option<String>, GitError> {
    let sha = git
        .cmd(Sub::RevParse)
        .args(["--verify", "-q", "--end-of-options"])
        .arg(format!("{rev}^{{commit}}"))
        .ok_codes(&[0, 1])
        .line()?;
    Ok(Some(sha).filter(|s| !s.is_empty()))
}

fn ref_exists(git: &Git, full: &str) -> Result<bool, GitError> {
    Ok(verify(git, full)?.is_some())
}

fn symbolic_ref(git: &Git, name: &str) -> Result<Option<String>, GitError> {
    let r = git
        .cmd(Sub::SymbolicRef)
        .args(["-q", name])
        .ok_codes(&[0, 1, 128])
        .line()?;
    Ok(Some(r).filter(|s| !s.is_empty()))
}

pub fn count(git: &Git, range: &str) -> Result<usize, GitError> {
    Ok(git
        .cmd(Sub::RevList)
        .args(["--count", range])
        .line()?
        .parse()
        .unwrap_or(0))
}

fn merge_base(git: &Git, a: &str, b: &str) -> Result<Option<String>, GitError> {
    let mb = git
        .cmd(Sub::MergeBase)
        .args([a, b])
        .ok_codes(&[0, 1])
        .line()?;
    Ok(Some(mb).filter(|s| !s.is_empty()))
}

/// Remote names, from `remote.<name>.url` entries.
fn remotes(git: &Git) -> Result<Vec<String>, GitError> {
    let out = git
        .cmd(Sub::Config)
        .args(["-z", "--get-regexp", r"^remote\..*\.url$"])
        .ok_codes(&[0, 1])
        .out()?;
    let mut names: Vec<String> = out
        .split_str("\0")
        .filter_map(|e| {
            let key = e.split_str("\n").next()?.to_str().ok()?;
            Some(
                key.strip_prefix("remote.")?
                    .strip_suffix(".url")?
                    .to_owned(),
            )
        })
        .collect();
    names.dedup();
    Ok(names)
}

fn short(full: &str) -> &str {
    full.strip_prefix("refs/heads/")
        .or_else(|| full.strip_prefix("refs/remotes/"))
        .unwrap_or(full)
}

struct DefaultBranch {
    name: String,
    remote: Option<String>,
}

fn default_branch(git: &Git) -> Result<Option<DefaultBranch>, GitError> {
    let remotes = remotes(git)?;
    let remote = if remotes.iter().any(|r| r == "origin") {
        Some("origin".to_owned())
    } else if remotes.len() == 1 {
        Some(remotes[0].clone())
    } else {
        None
    };
    if let Some(r) = &remote {
        let head = format!("refs/remotes/{r}/HEAD");
        if let Some(target) = symbolic_ref(git, &head)? {
            if let Some(name) = target.strip_prefix(&format!("refs/remotes/{r}/")) {
                return Ok(Some(DefaultBranch {
                    name: name.to_owned(),
                    remote,
                }));
            }
        }
    }
    for name in ["main", "master"] {
        let local = ref_exists(git, &format!("refs/heads/{name}"))?;
        let tracking = match &remote {
            Some(r) => ref_exists(git, &format!("refs/remotes/{r}/{name}"))?,
            None => false,
        };
        if local || tracking {
            return Ok(Some(DefaultBranch {
                name: name.to_owned(),
                remote,
            }));
        }
    }
    Ok(None)
}

fn upstream_of(git: &Git, branch_ref: &str) -> Result<Option<String>, GitError> {
    let up = git
        .cmd(Sub::ForEachRef)
        .args(["--format=%(upstream)", branch_ref])
        .line()?;
    if up.is_empty() || !ref_exists(git, &up)? {
        return Ok(None);
    }
    Ok(Some(up))
}

/// The last `depth` commits, with Σ starting below the oldest.
fn recent(git: &Git, info: &mut BaseInfo, depth: usize, empty_tree: &str) -> Result<(), GitError> {
    let total = count(git, "HEAD")?;
    info.range = LogRange::Last(depth);
    info.count = total.min(depth);
    info.merge_base = if total > depth {
        verify(git, &format!("HEAD~{depth}"))?
    } else {
        Some(empty_tree.to_owned())
    };
    Ok(())
}

fn fmt_count(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Compare HEAD against `base_full` (a resolvable ref or rev).
fn against(
    git: &Git,
    info: &mut BaseInfo,
    base_full: &str,
    opts: &BaseOptions<'_>,
) -> Result<bool, GitError> {
    let Some(mb) = merge_base(git, base_full, "HEAD")? else {
        info.notes.push(format!(
            "{} shares no history with HEAD · use --base",
            short(base_full)
        ));
        return Ok(false);
    };
    info.merge_base = Some(mb);
    info.count = count(git, &format!("{base_full}..HEAD"))?;
    info.range = LogRange::Since(base_full.to_owned());
    if info.truncated() {
        info.notes.push(format!(
            "base {} is {} commits behind · wrong base? use --base",
            short(base_full),
            fmt_count(info.count)
        ));
    }
    let _ = opts;
    Ok(true)
}

pub fn resolve(git: &Git, opts: &BaseOptions<'_>) -> Result<BaseInfo, GitError> {
    let head = verify(git, "HEAD")?;
    let branch_ref = symbolic_ref(git, "HEAD")?.filter(|r| r.starts_with("refs/heads/"));
    let branch = branch_ref.as_deref().map(|r| short(r).to_owned());
    let mut info = BaseInfo {
        head: head.clone(),
        branch: branch.clone(),
        mode: BaseMode::Unborn,
        base_ref: None,
        merge_base: None,
        range: LogRange::None,
        count: 0,
        notes: Vec::new(),
        hint: None,
    };
    if head.is_none() {
        return Ok(info);
    }

    // 1. Explicit base.
    if let Some(explicit) = opts.explicit {
        if verify(git, explicit)?.is_some() {
            info.mode = BaseMode::Explicit;
            info.base_ref = Some(explicit.to_owned());
            if against(git, &mut info, explicit, opts)? {
                return Ok(info);
            }
        } else {
            info.notes.push(format!(
                "base {explicit:?} not found · using automatic base"
            ));
        }
    }

    // 2. Default branch.
    let default = default_branch(git)?;

    // 3. Trunk mode.
    if let (Some(d), Some(b)) = (&default, &branch) {
        if &d.name == b {
            let branch_ref = branch_ref.as_deref().unwrap_or_default();
            if let Some(up) = upstream_of(git, branch_ref)? {
                info.mode = BaseMode::TrunkUpstream;
                info.base_ref = Some(short(&up).to_owned());
                if against(git, &mut info, &up, opts)? {
                    info.notes.push(format!(
                        "trunk mode · unpushed commits ({}..HEAD)",
                        short(&up)
                    ));
                    if info.count == 0 {
                        info.hint =
                            Some("nothing unpushed · --base HEAD~10 to look further back".into());
                    }
                    return Ok(info);
                }
            }
            info.mode = BaseMode::TrunkRecent;
            info.base_ref = None;
            recent(git, &mut info, opts.trunk_depth, opts.empty_tree)?;
            info.notes.push(format!(
                "trunk mode · no upstream · last {} commits",
                opts.trunk_depth
            ));
            return Ok(info);
        }
    }

    // 4. Feature branch or detached HEAD: the closest candidate wins.
    let mut candidates: Vec<String> = Vec::new();
    if let Some(d) = &default {
        if let Some(r) = &d.remote {
            candidates.push(format!("refs/remotes/{r}/{}", d.name));
        }
        candidates.push(format!("refs/heads/{}", d.name));
    }
    for c in [
        "refs/remotes/origin/main",
        "refs/heads/main",
        "refs/remotes/origin/master",
        "refs/heads/master",
    ] {
        candidates.push(c.to_owned());
    }
    let mut seen = Vec::new();
    let mut best: Option<(usize, String)> = None;
    for c in candidates {
        if seen.contains(&c) || Some(&c) == branch_ref.as_ref() {
            continue;
        }
        seen.push(c.clone());
        if !ref_exists(git, &c)? {
            continue;
        }
        let n = count(git, &format!("{c}..HEAD"))?;
        if best.as_ref().is_none_or(|(m, _)| n < *m) {
            best = Some((n, c));
        }
    }
    if let Some((_, base)) = best {
        info.mode = BaseMode::Feature;
        info.base_ref = Some(short(&base).to_owned());
        if against(git, &mut info, &base, opts)? {
            return Ok(info);
        }
    }

    info.mode = BaseMode::NoBase;
    info.base_ref = None;
    recent(git, &mut info, opts.trunk_depth, opts.empty_tree)?;
    info.notes.push(format!(
        "no base branch found · last {} commits · use --base",
        opts.trunk_depth
    ));
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_counts() {
        assert_eq!(fmt_count(7), "7");
        assert_eq!(fmt_count(1204), "1,204");
        assert_eq!(fmt_count(1234567), "1,234,567");
    }
}

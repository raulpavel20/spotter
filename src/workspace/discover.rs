//! Which repositories to show: the ones given, or, for a folder that is
//! not a repository itself (a "portal" of services), the ones inside it.

use std::collections::{HashSet, VecDeque};
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use crate::git::Repo;
use crate::git::repo::DiscoverError;

/// Never searched: dependencies, build output, environments. Hidden
/// folders (`.venv`, `.cache`…) are skipped too.
const SKIP: &[&str] = &["node_modules", "target", "vendor", "venv", "__pycache__"];

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// How many folders down to look.
    pub depth: usize,
    /// The most repositories to open.
    pub max: usize,
    /// How long to look.
    pub budget: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            depth: 3,
            max: 32,
            budget: Duration::from_secs(2),
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Scan {
    /// Folders holding a `.git`, as reached from the root (a symlink keeps
    /// its own name), sorted.
    pub found: Vec<PathBuf>,
    /// More than `Limits::max` were found; the search stopped.
    pub too_many: bool,
    /// The search ran out of time; some may be missing.
    pub timed_out: bool,
}

/// Looks for repositories under `root`, without running git. It doesn't
/// look inside the repositories it finds (their nested repositories and
/// submodules belong to them), follows symlinks once, and skips bare
/// repositories.
pub fn scan(root: &Path, limits: &Limits) -> Scan {
    let started = Instant::now();
    let mut scan = Scan::default();
    let mut seen: HashSet<PathBuf> = fs::canonicalize(root).into_iter().collect();
    let mut queue = VecDeque::from([(root.to_path_buf(), 0)]);
    while let Some((dir, depth)) = queue.pop_front() {
        if started.elapsed() > limits.budget {
            scan.timed_out = true;
            break;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            if name.starts_with('.') || SKIP.contains(&name.as_ref()) || !p.is_dir() {
                continue;
            }
            // Symlinks lead to folders seen before, or round in circles.
            let Ok(real) = fs::canonicalize(&p) else {
                continue;
            };
            if !seen.insert(real) {
                continue;
            }
            if p.join(".git").symlink_metadata().is_ok() {
                scan.found.push(p);
                if scan.found.len() > limits.max {
                    scan.too_many = true;
                    return scan;
                }
            } else if !looks_bare(&p) && depth + 1 < limits.depth {
                queue.push_back((p, depth + 1));
            }
        }
    }
    scan.found.sort();
    scan
}

fn looks_bare(p: &Path) -> bool {
    p.join("HEAD").is_file() && p.join("objects").is_dir() && p.join("refs").is_dir()
}

/// Tab names: each path's last folder, with parent folders added until
/// every name is unique (`api`, or `a/api` and `b/api`).
pub fn names(paths: &[PathBuf]) -> Vec<String> {
    let parts: Vec<Vec<String>> = paths
        .iter()
        .map(|p| {
            p.components()
                .filter_map(|c| match c {
                    Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                    _ => None,
                })
                .collect()
        })
        .collect();
    let mut take = vec![1; paths.len()];
    loop {
        let names: Vec<String> = parts
            .iter()
            .zip(&take)
            .map(|(p, &n)| match p.len() {
                0 => "/".to_owned(),
                len => p[len - n.min(len)..].join("/"),
            })
            .collect();
        let mut longer = false;
        for i in 0..names.len() {
            let clash = names
                .iter()
                .enumerate()
                .any(|(j, n)| j != i && *n == names[i]);
            if clash && take[i] < parts[i].len() {
                take[i] += 1;
                longer = true;
            }
        }
        if !longer {
            return names;
        }
    }
}

/// A repository to open, and its tab's name.
#[derive(Debug)]
pub struct Found {
    pub repo: Repo,
    pub name: String,
}

#[derive(Debug)]
pub struct Opened {
    pub repos: Vec<Found>,
    /// The folder searched, when the repositories came from one.
    pub folder: Option<PathBuf>,
    /// Worth telling once the screen is up (skipped repositories…).
    pub notes: Vec<String>,
}

#[derive(Debug)]
pub enum OpenError {
    NoSuchDir(PathBuf),
    NoRepos { dir: PathBuf, depth: usize },
    TooMany { dir: PathBuf, max: usize },
    Discover(DiscoverError),
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenError::NoSuchDir(p) => write!(f, "no such directory: {}", p.display()),
            OpenError::NoRepos { dir, depth } => write!(
                f,
                "no git repository in {} (searched {depth} levels down)",
                dir.display()
            ),
            OpenError::TooMany { dir, max } => {
                write!(f, "more than {max} git repositories in {}", dir.display())
            }
            OpenError::Discover(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for OpenError {}

impl OpenError {
    /// What to do about it.
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            OpenError::NoRepos { .. } => {
                Some("run spotter in a repository, or in a folder that holds some")
            }
            OpenError::TooMany { .. } => Some("pass the ones you want instead: spotter api web …"),
            _ => None,
        }
    }
}

pub fn open(paths: &[PathBuf], limits: &Limits) -> Result<Opened, OpenError> {
    open_with_env(paths, limits, Vec::new())
}

/// [`open`] with extra environment for git (tests isolate config).
pub fn open_with_env(
    paths: &[PathBuf],
    limits: &Limits,
    env: Vec<(OsString, OsString)>,
) -> Result<Opened, OpenError> {
    let mut found: Vec<(PathBuf, Repo)> = Vec::new();
    let mut notes = Vec::new();
    let mut folder = None;
    let several = paths.len() > 1;
    for path in paths {
        match Repo::discover_with_env(path, env.clone()) {
            Ok(r) => found.push((r.root.clone(), r)),
            Err(DiscoverError::NotARepo(_)) if !path.is_dir() => {
                return Err(OpenError::NoSuchDir(path.clone()));
            }
            Err(DiscoverError::NotARepo(_)) if searchable(path) => {
                let scan = scan(path, limits);
                if scan.too_many {
                    return Err(OpenError::TooMany {
                        dir: path.clone(),
                        max: limits.max,
                    });
                }
                if scan.timed_out {
                    notes.push(format!(
                        "stopped looking in {} after {}s · some repositories may be missing",
                        path.display(),
                        limits.budget.as_secs()
                    ));
                }
                let repos = discover_all(&scan.found, &env, &mut notes)?;
                if repos.is_empty() {
                    if several {
                        notes.push(format!("no git repository in {}", path.display()));
                        continue;
                    }
                    return Err(OpenError::NoRepos {
                        dir: path.clone(),
                        depth: limits.depth,
                    });
                }
                if !several {
                    folder = Some(fs::canonicalize(path).unwrap_or_else(|_| path.clone()));
                }
                found.extend(repos);
            }
            Err(e) => return Err(OpenError::Discover(e)),
        }
    }
    // The same repository twice (`spotter . ./src`, a symlink): once.
    let mut roots = HashSet::new();
    found.retain(|(_, r)| roots.insert(r.root.clone()));
    if found.is_empty() {
        let dir = paths.first().cloned().unwrap_or_default();
        return Err(OpenError::NoRepos {
            dir,
            depth: limits.depth,
        });
    }
    if found.len() > limits.max {
        return Err(OpenError::TooMany {
            dir: paths.first().cloned().unwrap_or_default(),
            max: limits.max,
        });
    }
    let shown: Vec<PathBuf> = found.iter().map(|(p, _)| p.clone()).collect();
    let names = names(&shown);
    if let (Some(dir), [_]) = (&folder, found.as_slice()) {
        let dir = dir.file_name().unwrap_or_default().to_string_lossy();
        notes.insert(
            0,
            format!("opened {}, the only repository in {dir}", names[0]),
        );
        folder = None;
    }
    Ok(Opened {
        repos: found
            .into_iter()
            .zip(names)
            .map(|((_, repo), name)| Found { repo, name })
            .collect(),
        folder,
        notes,
    })
}

/// Search a folder git says isn't a repository? Not from inside a `.git`
/// directory, and not when `GIT_DIR` points somewhere on purpose.
fn searchable(path: &Path) -> bool {
    let real = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    std::env::var_os("GIT_DIR").is_none() && !real.components().any(|c| c.as_os_str() == ".git")
}

/// Opens the candidates in parallel. Broken ones are skipped with a note;
/// if none opens, the first failure is the answer.
fn discover_all(
    paths: &[PathBuf],
    env: &[(OsString, OsString)],
    notes: &mut Vec<String>,
) -> Result<Vec<(PathBuf, Repo)>, OpenError> {
    let results: Vec<Result<Repo, DiscoverError>> = std::thread::scope(|s| {
        let handles: Vec<_> = paths
            .iter()
            .map(|p| s.spawn(move || Repo::discover_with_env(p, env.to_vec())))
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err(DiscoverError::NotARepo(PathBuf::new())))
            })
            .collect()
    });
    let mut repos = Vec::new();
    let mut first_err = None;
    for (p, r) in paths.iter().zip(results) {
        match r {
            Ok(repo) => repos.push((p.clone(), repo)),
            Err(e) => {
                crate::log::line(|| format!("skipped {}: {e}", p.display()));
                notes.push(format!("skipped {}: {e}", p.display()));
                first_err.get_or_insert(e);
            }
        }
    }
    match (repos.is_empty(), first_err) {
        (true, Some(e)) => Err(OpenError::Discover(e)),
        _ => Ok(repos),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(root: &Path, rel: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.join(".git")).unwrap();
    }

    fn rel(root: &Path, s: &Scan) -> Vec<String> {
        s.found
            .iter()
            .map(|p| p.strip_prefix(root).unwrap().display().to_string())
            .collect()
    }

    #[test]
    fn scans_a_portal_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        repo(root, "web");
        repo(root, "api");
        repo(root, "services/worker");
        repo(root, "a/b/deep");
        repo(root, "a/b/c/too-deep");
        // Inside a repository: its own business.
        repo(root, "api/nested");
        // Skipped folders.
        repo(root, "node_modules/pkg");
        repo(root, ".hidden/repo");
        repo(root, "target/x");
        // A worktree or submodule: `.git` is a file.
        fs::create_dir_all(root.join("wt")).unwrap();
        fs::write(root.join("wt/.git"), "gitdir: /elsewhere\n").unwrap();
        // A bare repository is not opened, nor searched.
        for d in ["bare.git/objects", "bare.git/refs", "bare.git/inner/.git"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        fs::write(root.join("bare.git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(root.join("notes.txt"), "").unwrap();

        let s = scan(root, &Limits::default());
        assert_eq!(
            rel(root, &s),
            ["a/b/deep", "api", "services/worker", "web", "wt"]
        );
        assert!(!s.too_many && !s.timed_out);

        let shallow = Limits {
            depth: 1,
            ..Limits::default()
        };
        assert_eq!(rel(root, &scan(root, &shallow)), ["api", "web", "wt"]);
    }

    #[test]
    fn stops_when_there_are_too_many() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..5 {
            repo(tmp.path(), &format!("r{i}"));
        }
        let limits = Limits {
            max: 3,
            ..Limits::default()
        };
        let s = scan(tmp.path(), &limits);
        assert!(s.too_many);
        assert_eq!(s.found.len(), 4);
        let at_max = Limits {
            max: 5,
            ..Limits::default()
        };
        assert!(!scan(tmp.path(), &at_max).too_many);
    }

    #[cfg(unix)]
    #[test]
    fn follows_symlinks_once() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("portal");
        repo(&tmp.path().join("elsewhere"), "svc");
        fs::create_dir_all(&root).unwrap();
        symlink(tmp.path().join("elsewhere/svc"), root.join("linked")).unwrap();
        repo(&root, "real");
        symlink(root.join("real"), root.join("same")).unwrap();
        // A loop back up.
        fs::create_dir_all(root.join("group")).unwrap();
        symlink(&root, root.join("group/up")).unwrap();
        let s = scan(&root, &Limits::default());
        assert_eq!(rel(&root, &s), ["linked", "real"]);
    }

    #[test]
    fn names_are_short_and_unique() {
        let p = |s: &str| PathBuf::from(s);
        assert_eq!(
            names(&[p("/w/api"), p("/w/web"), p("/w/services/worker")]),
            ["api", "web", "worker"]
        );
        assert_eq!(
            names(&[p("/w/a/api"), p("/w/b/api"), p("/w/web")]),
            ["a/api", "b/api", "web"]
        );
        assert_eq!(
            names(&[p("/x/a/api"), p("/y/a/api")]),
            ["x/a/api", "y/a/api"]
        );
        assert_eq!(names(&[p("/")]), ["/"]);
    }
}

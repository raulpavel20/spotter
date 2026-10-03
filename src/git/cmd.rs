//! The single choke point for running git.
//!
//! Every git invocation goes through [`Git::cmd`]. Subcommands are a closed
//! enum, so nothing outside the read-only allowlist can be spawned, and the
//! fixed global arguments and environment are always applied.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Global arguments that neutralise user configuration we would otherwise
/// have to parse around.
pub const GLOBAL_ARGS: &[&str] = &[
    "--no-optional-locks",
    "--no-pager",
    "--literal-pathspecs",
    "-c",
    "core.quotepath=off",
    "-c",
    "color.ui=never",
    "-c",
    "diff.noprefix=false",
    "-c",
    "diff.mnemonicPrefix=false",
    "-c",
    "diff.relative=false",
    "-c",
    "log.showSignature=false",
    // Porcelain `git diff` otherwise rewrites the index when files are
    // stat-dirty, even with --no-optional-locks.
    "-c",
    "diff.autoRefreshIndex=false",
];

/// Arguments added right after `diff`, `log` and `show`.
pub const DIFF_ARGS: &[&str] = &[
    "--no-ext-diff",
    "--no-textconv",
    "--no-color",
    "--src-prefix=a/",
    "--dst-prefix=b/",
    "--submodule=short",
    "-M",
];

/// The read-only allowlist. There is deliberately no way to name any other
/// subcommand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sub {
    Version,
    RevParse,
    SymbolicRef,
    ForEachRef,
    RevList,
    MergeBase,
    Log,
    Show,
    Diff,
    Status,
    CatFile,
    CheckAttr,
    HashObject,
    Config,
}

impl Sub {
    pub const ALL: &'static [Sub] = &[
        Sub::Version,
        Sub::RevParse,
        Sub::SymbolicRef,
        Sub::ForEachRef,
        Sub::RevList,
        Sub::MergeBase,
        Sub::Log,
        Sub::Show,
        Sub::Diff,
        Sub::Status,
        Sub::CatFile,
        Sub::CheckAttr,
        Sub::HashObject,
        Sub::Config,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Sub::Version => "version",
            Sub::RevParse => "rev-parse",
            Sub::SymbolicRef => "symbolic-ref",
            Sub::ForEachRef => "for-each-ref",
            Sub::RevList => "rev-list",
            Sub::MergeBase => "merge-base",
            Sub::Log => "log",
            Sub::Show => "show",
            Sub::Diff => "diff",
            Sub::Status => "status",
            Sub::CatFile => "cat-file",
            Sub::CheckAttr => "check-attr",
            Sub::HashObject => "hash-object",
            Sub::Config => "config",
        }
    }

    fn takes_diff_args(self) -> bool {
        matches!(self, Sub::Log | Sub::Show | Sub::Diff)
    }
}

/// A failed git invocation. Shown on the status line, never a panic.
#[derive(Debug, Clone)]
pub struct GitError {
    pub command: String,
    pub code: Option<i32>,
    pub stderr: String,
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let stderr = self.stderr.trim();
        match self.code {
            Some(code) => write!(f, "{} failed (exit {code})", self.command)?,
            None => write!(f, "{} failed", self.command)?,
        }
        if !stderr.is_empty() {
            let first = stderr.lines().next().unwrap_or_default();
            write!(f, ": {first}")?;
        }
        Ok(())
    }
}

impl std::error::Error for GitError {}

/// Handle used to run git in one repository.
#[derive(Debug, Clone)]
pub struct Git {
    dir: PathBuf,
    extra_env: Vec<(OsString, OsString)>,
}

impl Git {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Git {
            dir: dir.into(),
            extra_env: Vec::new(),
        }
    }

    /// Extra environment for every call. Tests use this to isolate config
    /// (`GIT_CONFIG_GLOBAL`, `GIT_CONFIG_NOSYSTEM`) without touching the
    /// process environment.
    pub fn with_env(mut self, env: Vec<(OsString, OsString)>) -> Self {
        self.extra_env = env;
        self
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn set_dir(&mut self, dir: impl Into<PathBuf>) {
        self.dir = dir.into();
    }

    pub fn extra_env(&self) -> &[(OsString, OsString)] {
        &self.extra_env
    }

    pub fn cmd(&self, sub: Sub) -> Cmd<'_> {
        Cmd {
            git: self,
            sub,
            args: Vec::new(),
            stdin: None,
            ok_codes: vec![0],
        }
    }
}

/// A git invocation under construction.
pub struct Cmd<'g> {
    git: &'g Git,
    sub: Sub,
    args: Vec<OsString>,
    stdin: Option<Vec<u8>>,
    ok_codes: Vec<i32>,
}

/// Output of a successful call. `code` is one of the call's ok codes.
#[derive(Debug, Clone)]
pub struct Output {
    pub stdout: Vec<u8>,
    pub code: i32,
}

impl Cmd<'_> {
    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(args.into_iter().map(|a| a.as_ref().to_owned()));
        self
    }

    pub fn stdin(mut self, input: Vec<u8>) -> Self {
        self.stdin = Some(input);
        self
    }

    /// Exit codes that count as success for this call, e.g. `[0, 1]` for
    /// `diff --no-index` or `rev-parse --verify -q`.
    pub fn ok_codes(mut self, codes: &[i32]) -> Self {
        self.ok_codes = codes.to_vec();
        self
    }

    /// The full argument vector after `git`, as it will be spawned.
    pub fn argv(&self) -> Vec<OsString> {
        let mut argv: Vec<OsString> = vec!["-C".into(), self.git.dir.clone().into_os_string()];
        argv.extend(GLOBAL_ARGS.iter().map(OsString::from));
        argv.push(self.sub.name().into());
        if self.sub.takes_diff_args() {
            argv.extend(DIFF_ARGS.iter().map(OsString::from));
            if self.sub == Sub::Log {
                argv.push("--root".into());
            }
        }
        argv.extend(self.args.iter().cloned());
        argv
    }

    fn display(&self) -> String {
        let mut s = format!("git {}", self.sub.name());
        for a in self.args.iter().take(4) {
            s.push(' ');
            s.push_str(&a.to_string_lossy());
        }
        if self.args.len() > 4 {
            s.push_str(" …");
        }
        s
    }

    pub fn run(self) -> Result<Output, GitError> {
        let started = std::time::Instant::now();
        let result = self.run_inner();
        crate::log::line(|| {
            let args: Vec<String> = self
                .args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            let what = format!("git {} {}", self.sub.name(), args.join(" "));
            let ms = started.elapsed().as_millis();
            match &result {
                Ok(o) => format!("{what} -> {} in {ms}ms, {} bytes", o.code, o.stdout.len()),
                Err(e) => format!("{what} -> FAILED in {ms}ms: {e}"),
            }
        });
        result
    }

    fn run_inner(&self) -> Result<Output, GitError> {
        if let Err(reason) = validate(self.sub, &self.args) {
            return Err(GitError {
                command: self.display(),
                code: None,
                stderr: format!("refused by spotter: {reason}"),
            });
        }
        let mut cmd = Command::new("git");
        cmd.args(self.argv())
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(if self.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            });
        for (k, v) in &self.git.extra_env {
            cmd.env(k, v);
        }
        let err = |code, stderr: String| GitError {
            command: self.display(),
            code,
            stderr,
        };
        let mut child = cmd
            .spawn()
            .map_err(|e| err(None, format!("could not run git: {e}")))?;
        if let Some(input) = &self.stdin {
            let mut pipe = child.stdin.take().expect("stdin is piped");
            let input = input.clone();
            // Write on a separate thread so a large output can't deadlock us.
            std::thread::spawn(move || {
                let _ = pipe.write_all(&input);
            });
        }
        let out = child
            .wait_with_output()
            .map_err(|e| err(None, format!("waiting for git: {e}")))?;
        let code = out.status.code();
        match code {
            Some(c) if self.ok_codes.contains(&c) => Ok(Output {
                stdout: out.stdout,
                code: c,
            }),
            _ => Err(err(code, String::from_utf8_lossy(&out.stderr).into_owned())),
        }
    }

    /// Run and return stdout.
    pub fn out(self) -> Result<Vec<u8>, GitError> {
        self.run().map(|o| o.stdout)
    }

    /// Run and return stdout as a trimmed, lossily decoded string.
    pub fn line(self) -> Result<String, GitError> {
        self.out()
            .map(|o| String::from_utf8_lossy(&o).trim_end().to_owned())
    }
}

/// Argument checks for the subcommands that have write modes.
fn validate(sub: Sub, args: &[OsString]) -> Result<(), String> {
    match sub {
        Sub::HashObject => {
            // A strict allowlist of options; anything after `--` is a path.
            let mut expect_type = false;
            for a in args {
                let a = a.to_string_lossy();
                if expect_type {
                    if a != "blob" && a != "tree" {
                        return Err(format!("hash-object type {a:?}"));
                    }
                    expect_type = false;
                    continue;
                }
                match a.as_ref() {
                    "--" => return Ok(()),
                    "-t" => expect_type = true,
                    "--stdin" | "--stdin-paths" => {}
                    _ if a.starts_with("--path=") => {}
                    _ => return Err(format!("hash-object option {a:?}")),
                }
            }
            Ok(())
        }
        Sub::Config => {
            const DENY: &[&str] = &[
                "--add",
                "--unset",
                "--unset-all",
                "--replace-all",
                "--edit",
                "-e",
                "--remove-section",
                "--rename-section",
            ];
            let mut has_get = false;
            for a in args {
                let a = a.to_string_lossy();
                if DENY.contains(&a.as_ref()) {
                    return Err(format!("config option {a:?}"));
                }
                if a.starts_with("--get") {
                    has_get = true;
                }
            }
            if has_get {
                Ok(())
            } else {
                Err("config without --get".into())
            }
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_is_exactly_the_read_only_set() {
        let names: Vec<_> = Sub::ALL.iter().map(|s| s.name()).collect();
        assert_eq!(
            names,
            [
                "version",
                "rev-parse",
                "symbolic-ref",
                "for-each-ref",
                "rev-list",
                "merge-base",
                "log",
                "show",
                "diff",
                "status",
                "cat-file",
                "check-attr",
                "hash-object",
                "config",
            ]
        );
    }

    #[test]
    fn subcommand_follows_global_args_and_user_args_come_last() {
        let git = Git::new("/repo");
        for &sub in Sub::ALL {
            let argv = git.cmd(sub).arg("reset").arg("--hard").argv();
            let argv: Vec<_> = argv.iter().map(|a| a.to_string_lossy()).collect();
            assert_eq!(argv[0], "-C");
            assert_eq!(argv[1], "/repo");
            assert_eq!(argv[2..2 + GLOBAL_ARGS.len()], *GLOBAL_ARGS);
            assert_eq!(argv[2 + GLOBAL_ARGS.len()], sub.name());
            assert_eq!(argv[argv.len() - 2..], ["reset", "--hard"]);
        }
    }

    #[test]
    fn diff_family_gets_safety_args() {
        let git = Git::new("/repo");
        let argv: Vec<_> = git
            .cmd(Sub::Log)
            .argv()
            .into_iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        for a in DIFF_ARGS {
            assert!(argv.iter().any(|x| x == a), "missing {a}");
        }
        assert!(argv.iter().any(|x| x == "--root"));
        let status: Vec<_> = git.cmd(Sub::Status).argv();
        assert!(!status.iter().any(|a| a == "--no-ext-diff"));
    }

    #[test]
    fn hash_object_write_is_refused() {
        let git = Git::new("/nonexistent");
        for bad in [&["-w"][..], &["--write"], &["-t", "blob", "-w"], &["-tw"]] {
            let err = git.cmd(Sub::HashObject).args(bad).run().unwrap_err();
            assert!(err.stderr.contains("refused"), "{bad:?}: {err}");
        }
        assert!(validate(Sub::HashObject, &["--stdin-paths".into()]).is_ok());
        assert!(
            validate(
                Sub::HashObject,
                &["-t".into(), "tree".into(), "--stdin".into()]
            )
            .is_ok()
        );
        assert!(validate(Sub::HashObject, &["--".into(), "-w".into()]).is_ok());
    }

    #[test]
    fn config_writes_are_refused() {
        let ok = |args: &[&str]| {
            validate(
                Sub::Config,
                &args.iter().map(OsString::from).collect::<Vec<_>>(),
            )
        };
        assert!(ok(&["user.name", "x"]).is_err());
        assert!(ok(&["--add", "a.b", "c"]).is_err());
        assert!(ok(&["--get", "--unset", "a.b"]).is_err());
        assert!(ok(&["-z", "--get-regexp", "^spotter\\."]).is_ok());
        assert!(ok(&["--get", "a.b"]).is_ok());
    }
}

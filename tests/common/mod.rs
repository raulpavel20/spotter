//! Throwaway repositories for integration tests (PLAN §10).
#![allow(dead_code)]

use std::cell::Cell;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use spotter::config::Config;
use spotter::git::Repo;
use spotter::refresh::{self, Cache, RefreshOpts, Snapshot};
use tempfile::TempDir;

pub struct TestRepo {
    pub tmp: TempDir,
    pub path: PathBuf,
    global: PathBuf,
    clock: Cell<u64>,
}

impl TestRepo {
    pub fn new() -> Self {
        Self::init(&[])
    }

    pub fn sha256() -> Self {
        Self::init(&["--object-format=sha256"])
    }

    fn init(extra: &[&str]) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("repo");
        fs::create_dir(&path).unwrap();
        let global = tmp.path().join("gitconfig");
        fs::write(
            &global,
            "[user]\n\tname = Test\n\temail = test@example.com\n[init]\n\tdefaultBranch = main\n\
             [commit]\n\tgpgsign = false\n[advice]\n\tdetachedHead = false\n[core]\n\tautocrlf = false\n",
        )
        .unwrap();
        let repo = TestRepo {
            tmp,
            path,
            global,
            clock: Cell::new(1_700_000_000),
        };
        let mut args = vec!["init", "-q", "-b", "main"];
        args.extend_from_slice(extra);
        repo.git(&args);
        repo
    }

    /// Isolated environment: no user or system config leaks in.
    pub fn env(&self) -> Vec<(OsString, OsString)> {
        vec![
            ("GIT_CONFIG_GLOBAL".into(), self.global.clone().into()),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        ]
    }

    pub fn git_in(&self, dir: &Path, args: &[&str]) -> String {
        let t = self.clock.get() + 1;
        self.clock.set(t);
        let date = format!("{t} +0000");
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .envs(self.env())
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .env("HOME", self.tmp.path())
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim_end().to_owned()
    }

    pub fn git(&self, args: &[&str]) -> String {
        self.git_in(&self.path, args)
    }

    /// Like `git` but tolerates failure (e.g. a conflicting merge).
    pub fn git_may_fail(&self, args: &[&str]) -> bool {
        Command::new("git")
            .args(args)
            .current_dir(&self.path)
            .envs(self.env())
            .env("HOME", self.tmp.path())
            .output()
            .unwrap()
            .status
            .success()
    }

    pub fn write(&self, rel: &str, content: impl AsRef<[u8]>) {
        let p = self.path.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, content).unwrap();
    }

    pub fn remove(&self, rel: &str) {
        fs::remove_file(self.path.join(rel)).unwrap();
    }

    /// Stage everything and commit; returns the new HEAD.
    pub fn commit(&self, msg: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", msg]);
        self.head()
    }

    pub fn commit_file(&self, rel: &str, content: &str, msg: &str) -> String {
        self.write(rel, content);
        self.commit(msg)
    }

    pub fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }

    /// Many empty commits, fast.
    pub fn empty_commits(&self, n: usize, prefix: &str) {
        for i in 0..n {
            self.git(&[
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                &format!("{prefix} {i}"),
            ]);
        }
    }

    /// Creates a bare repo, adds it as `name`, pushes the current branch
    /// and points `<name>/HEAD` at it.
    pub fn add_remote(&self, name: &str) -> PathBuf {
        let branch = self.git(&["symbolic-ref", "--short", "HEAD"]);
        let bare = self.tmp.path().join(format!("{name}.git"));
        self.git_in(
            self.tmp.path(),
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                &branch,
                bare.to_str().unwrap(),
            ],
        );
        self.git(&["remote", "add", name, bare.to_str().unwrap()]);
        self.git(&["push", "-q", name, &branch]);
        self.git(&["fetch", "-q", name]);
        self.git(&["remote", "set-head", name, &branch]);
        bare
    }

    /// Settings that would break naive parsing (PLAN §10).
    pub fn hostile(&self) {
        let script = self.tmp.path().join("external-diff.sh");
        fs::write(&script, "#!/bin/sh\necho EXTERNAL DIFF\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut cfg = fs::read_to_string(&self.global).unwrap();
        cfg.push_str(&format!(
            "[diff]\n\tnoprefix = true\n\texternal = {}\n\trelative = true\n\trenames = false\n\
             \tmnemonicPrefix = true\n\tsubmodule = log\n\
             [color]\n\tui = always\n\tdiff = always\n\tstatus = always\n\
             [log]\n\tshowSignature = true\n\tshowRoot = false\n\tdecorate = full\n\
             [core]\n\tquotepath = true\n\
             [status]\n\tshowUntrackedFiles = no\n\trelativePaths = true\n\
             [pager]\n\tdiff = false\n",
            script.display()
        ));
        fs::write(&self.global, cfg).unwrap();
    }

    pub fn repo(&self) -> Repo {
        Repo::discover_with_env(&self.path, self.env()).expect("discover")
    }

    pub fn repo_at(&self, dir: &Path) -> Repo {
        Repo::discover_with_env(dir, self.env()).expect("discover")
    }

    pub fn snapshot(&self) -> Snapshot {
        self.snapshot_with(None, true)
    }

    pub fn snapshot_with(&self, base: Option<&str>, include_wt: bool) -> Snapshot {
        let repo = self.repo();
        let cfg = Config::load(&repo.git);
        let opts = RefreshOpts {
            include_wt,
            ..RefreshOpts::default()
        };
        refresh::full(&repo, &cfg, base, &opts, None, &mut Cache::default()).expect("refresh")
    }
}

/// Runs a scenario under both a clean and a hostile configuration.
pub fn both(scenario: impl Fn(&TestRepo) -> Box<dyn Fn(&TestRepo)>) {
    for hostile in [false, true] {
        let r = TestRepo::new();
        let check = scenario(&r);
        if hostile {
            r.hostile();
        }
        check(&r);
    }
}

pub fn paths(files: &[spotter::model::FileChange]) -> Vec<String> {
    files.iter().map(|f| f.display_path()).collect()
}

// ---------------------------------------------------------------------------
// Driving the App synchronously.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use spotter::app::App;
use spotter::msg::{EditRequest, Effect, Msg, RefreshKind, WatchStatus};
use spotter::review::Marks;
use spotter::worker::Worker;

pub const NOW: i64 = 1_800_000_000;

fn fixed_now() -> i64 {
    NOW
}

/// Runs the App's effects in place of the worker thread.
pub struct Harness {
    pub app: App,
    pub worker: Worker,
    pub repo: Repo,
    pub edits: Vec<EditRequest>,
}

impl Harness {
    /// Marks live in the repo's real store.
    pub fn new(r: &TestRepo) -> Self {
        let repo = r.repo();
        let marks = Marks::load(repo.marks_path());
        Self::with_marks(r, marks)
    }

    pub fn in_memory(r: &TestRepo) -> Self {
        Self::with_marks(r, Marks::in_memory())
    }

    fn with_marks(r: &TestRepo, marks: Marks) -> Self {
        let repo = r.repo();
        let cfg = Config::load(&repo.git);
        let app = App::new(repo.empty_tree.clone(), marks, WatchStatus::Live).with_clock(fixed_now);
        let worker = Worker::new(repo.clone(), cfg, None);
        let mut h = Harness {
            app,
            worker,
            repo,
            edits: Vec::new(),
        };
        let fx = h.app.start();
        h.run(fx);
        h
    }

    pub fn run(&mut self, mut fx: Vec<Effect>) {
        while !fx.is_empty() {
            let mut next = Vec::new();
            for e in fx {
                let msg = match e {
                    Effect::Refresh { seq, kind, opts } => Some(Msg::Refreshed {
                        seq,
                        result: self.worker.refresh(kind, &opts).map(Box::new),
                    }),
                    Effect::LoadPatch { seq, spec, files } => Some(Msg::PatchLoaded {
                        seq,
                        result: self.worker.patch(&spec, &files),
                    }),
                    Effect::LoadFile {
                        seq,
                        index,
                        spec,
                        file,
                    } => Some(Msg::FileLoaded {
                        seq,
                        index,
                        result: refresh::load_file(&self.repo, &spec, &file)
                            .map_err(|e| e.to_string()),
                    }),
                    Effect::SaveMarks => {
                        self.app.marks.save(NOW).unwrap();
                        None
                    }
                    Effect::OpenEditor(req) => {
                        self.edits.push(req);
                        None
                    }
                    Effect::Redraw | Effect::Quit => None,
                };
                if let Some(m) = msg {
                    next.extend(self.app.update(m));
                }
            }
            fx = next;
        }
    }

    pub fn send(&mut self, msg: Msg) {
        let fx = self.app.update(msg);
        self.run(fx);
    }

    /// Space-separated keys: `j`, `enter`, `tab`, `esc`, `space`, `ctrl-d`…
    pub fn keys(&mut self, spec: &str) {
        for k in spec.split_whitespace() {
            let (code, mods) = match k {
                "enter" => (KeyCode::Enter, KeyModifiers::NONE),
                "tab" => (KeyCode::Tab, KeyModifiers::NONE),
                "esc" => (KeyCode::Esc, KeyModifiers::NONE),
                "space" => (KeyCode::Char(' '), KeyModifiers::NONE),
                "down" => (KeyCode::Down, KeyModifiers::NONE),
                "pgdn" => (KeyCode::PageDown, KeyModifiers::NONE),
                s if s.starts_with("ctrl-") => (
                    KeyCode::Char(s.chars().last().unwrap()),
                    KeyModifiers::CONTROL,
                ),
                s => (KeyCode::Char(s.chars().next().unwrap()), KeyModifiers::NONE),
            };
            self.send(Msg::Key(KeyEvent::new(code, mods)));
        }
    }

    pub fn refresh(&mut self, kind: RefreshKind) {
        self.send(Msg::Fs(kind));
    }

    pub fn render(&mut self, w: u16, h: u16) -> String {
        self.send(Msg::Resize(w, h));
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| spotter::ui::draw(f, &mut self.app)).unwrap();
        let buf = t.backend().buffer();
        let mut out = String::new();
        for y in 0..h {
            let mut line = String::new();
            for x in 0..w {
                line.push_str(buf[(x, y)].symbol());
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }
}

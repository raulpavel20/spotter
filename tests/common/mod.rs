//! Throwaway repositories for integration tests.
#![allow(dead_code)]

use std::cell::Cell;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;

use spotter::config::Config;
use spotter::git::Repo;
use spotter::refresh::{self, Cache, RefreshOpts, Snapshot};
use tempfile::TempDir;

pub struct TestRepo {
    /// Shared by repositories made with [`TestRepo::sibling`].
    pub tmp: Rc<TempDir>,
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
        Self::init_at("repo", extra)
    }

    /// A repository at `rel` inside a fresh temporary folder
    /// (`portal/api`).
    pub fn at(rel: &str) -> Self {
        Self::init_at(rel, &[])
    }

    fn init_at(rel: &str, extra: &[&str]) -> Self {
        let tmp = Rc::new(tempfile::tempdir().unwrap());
        let global = tmp.path().join("gitconfig");
        fs::write(
            &global,
            "[user]\n\tname = Test\n\temail = test@example.com\n[init]\n\tdefaultBranch = main\n\
             [commit]\n\tgpgsign = false\n[advice]\n\tdetachedHead = false\n[core]\n\tautocrlf = false\n",
        )
        .unwrap();
        Self::create(tmp, global, rel, extra)
    }

    /// Another repository in the same temporary folder, with the same
    /// isolated git config.
    pub fn sibling(&self, rel: &str) -> Self {
        Self::create(self.tmp.clone(), self.global.clone(), rel, &[])
    }

    fn create(tmp: Rc<TempDir>, global: PathBuf, rel: &str, extra: &[&str]) -> Self {
        let path = tmp.path().join(rel);
        fs::create_dir_all(&path).unwrap();
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
        let mut bare = self.tmp.path().join(format!("{name}.git"));
        if bare.exists() {
            // A sibling's remote.
            let dir = self.path.file_name().unwrap().to_string_lossy();
            bare = self.tmp.path().join(format!("{dir}-{name}.git"));
        }
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

    /// Settings that would break naive parsing.
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
        let cfg = Config::load(None, &repo.git).config;
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
use spotter::highlight::Highlighter;
use spotter::msg::{EditRequest, Effect, Msg, RefreshKind, WatchStatus};
use spotter::review::Marks;
use spotter::worker::Worker;

pub const NOW: i64 = 1_800_000_000;

fn fixed_now() -> i64 {
    NOW
}

/// One highlighter per test binary: loading grammars takes a moment.
pub fn highlighter() -> &'static Highlighter {
    static HL: std::sync::OnceLock<Highlighter> = std::sync::OnceLock::new();
    HL.get_or_init(|| Highlighter::new(two_face::theme::EmbeddedThemeName::MonokaiExtended))
}

/// What the app asked for that tests look at instead of doing.
#[derive(Default)]
pub struct Recorded {
    pub edits: Vec<EditRequest>,
    /// Settings the app asked to save.
    pub saved: Vec<(&'static str, Option<spotter::config::Value>)>,
    /// Commit messages sent to the editor.
    pub messages: Vec<String>,
    /// Replies to password prompts.
    pub answers: Vec<(u64, Option<String>)>,
}

/// Runs one effect the way main would, synchronously; returns the
/// message that answers it. `ReloadConfig` reads `config` (the settings
/// file, if the test has one).
pub fn execute(
    worker: &mut Worker,
    repo: &Repo,
    app: &mut App,
    rec: &mut Recorded,
    config: Option<&Path>,
    e: Effect,
) -> Option<Msg> {
    match e {
        Effect::Refresh { seq, kind, opts } => Some(Msg::Refreshed {
            seq,
            result: worker.refresh(kind, &opts).map(Box::new),
        }),
        Effect::LoadPatch {
            seq,
            spec,
            files,
            opts,
        } => Some(Msg::PatchLoaded {
            seq,
            result: worker.patch(&spec, &files, opts),
        }),
        Effect::LoadFile {
            seq,
            index,
            spec,
            file,
            opts,
        } => Some(Msg::FileLoaded {
            seq,
            index,
            result: refresh::load_file(repo, &spec, &file, opts).map_err(|e| e.to_string()),
        }),
        Effect::SaveSetting { key, value } => {
            if let Some(p) = config {
                spotter::config_file::save(p, key, value.as_ref()).unwrap();
            }
            rec.saved.push((key, value));
            None
        }
        Effect::ReloadConfig => {
            let mut loaded = Config::load(config, &repo.git);
            loaded.config.syntax = app.config.syntax;
            Some(Msg::ConfigReloaded(Box::new(loaded)))
        }
        Effect::SetWorkerConfig(c) => {
            worker.set_config(*c);
            None
        }
        Effect::SetSyntaxTheme(_) | Effect::EditConfig => None,
        Effect::Highlight {
            index,
            key,
            path,
            patch,
            ..
        } => Some(Msg::Highlighted {
            index,
            key,
            hl: std::sync::Arc::new(highlighter().highlight(&path, &patch)),
        }),
        Effect::SaveMarks => {
            app.marks.save(NOW).unwrap();
            None
        }
        Effect::OpenEditor(req) => {
            rec.edits.push(req);
            None
        }
        Effect::Commit { files, message } => Some(Msg::Committed(spotter::git::write::commit(
            repo,
            &files,
            &message,
            &[],
        ))),
        Effect::PreparePush => Some(Msg::PushInfo(spotter::git::write::push_info(repo))),
        Effect::Push {
            remote,
            branch,
            dest,
            set_upstream,
        } => Some(Msg::Pushed(spotter::git::write::push(
            repo,
            &remote,
            &branch,
            &dest,
            set_upstream,
            &[],
        ))),
        Effect::EditMessage(m) => {
            rec.messages.push(m);
            None
        }
        Effect::Answer { id, answer } => {
            rec.answers.push((id, answer.map(|a| a.0)));
            None
        }
        Effect::Redraw | Effect::Quit => None,
    }
}

/// Space-separated keys: `j`, `enter`, `tab`, `esc`, `space`, `ctrl-d`…
pub fn key_events(spec: &str) -> Vec<KeyEvent> {
    spec.split_whitespace()
        .map(|k| {
            let (code, mods) = match k {
                "enter" => (KeyCode::Enter, KeyModifiers::NONE),
                "tab" => (KeyCode::Tab, KeyModifiers::NONE),
                "esc" => (KeyCode::Esc, KeyModifiers::NONE),
                "space" => (KeyCode::Char(' '), KeyModifiers::NONE),
                "down" => (KeyCode::Down, KeyModifiers::NONE),
                "pgdn" => (KeyCode::PageDown, KeyModifiers::NONE),
                "ctrl-pgdn" => (KeyCode::PageDown, KeyModifiers::CONTROL),
                "ctrl-pgup" => (KeyCode::PageUp, KeyModifiers::CONTROL),
                "left" => (KeyCode::Left, KeyModifiers::NONE),
                "right" => (KeyCode::Right, KeyModifiers::NONE),
                "backtab" => (KeyCode::BackTab, KeyModifiers::SHIFT),
                "backspace" => (KeyCode::Backspace, KeyModifiers::NONE),
                s if s.starts_with("ctrl-") => (
                    KeyCode::Char(s.chars().last().unwrap()),
                    KeyModifiers::CONTROL,
                ),
                s => (KeyCode::Char(s.chars().next().unwrap()), KeyModifiers::NONE),
            };
            KeyEvent::new(code, mods)
        })
        .collect()
}

/// Text, one key per character (a newline is Enter).
pub fn text_events(text: &str) -> Vec<KeyEvent> {
    text.chars()
        .map(|c| {
            let code = if c == '\n' {
                KeyCode::Enter
            } else {
                KeyCode::Char(c)
            };
            KeyEvent::new(code, KeyModifiers::NONE)
        })
        .collect()
}

/// The screen as text, trailing spaces trimmed.
pub fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
    let area = buf.area;
    let mut out = String::new();
    for y in 0..area.height {
        let mut line = String::new();
        for x in 0..area.width {
            line.push_str(buf[(x, y)].symbol());
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// Runs the App's effects in place of the worker thread.
pub struct Harness {
    pub app: App,
    pub worker: Worker,
    pub repo: Repo,
    pub rec: Recorded,
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

    /// In-memory marks and adjusted settings.
    pub fn configured(r: &TestRepo, f: impl FnOnce(&mut Config)) -> Self {
        Self::build(r, Marks::in_memory(), f)
    }

    fn with_marks(r: &TestRepo, marks: Marks) -> Self {
        Self::build(r, marks, |_| {})
    }

    fn build(r: &TestRepo, marks: Marks, f: impl FnOnce(&mut Config)) -> Self {
        let repo = r.repo();
        let (app, worker) = test_app(&repo, marks, None, f);
        let mut h = Harness {
            app,
            worker,
            repo,
            rec: Recorded::default(),
        };
        let fx = h.app.start();
        h.run(fx);
        h
    }

    pub fn run(&mut self, mut fx: Vec<Effect>) {
        while !fx.is_empty() {
            let mut next = Vec::new();
            for e in fx {
                let msg = execute(
                    &mut self.worker,
                    &self.repo,
                    &mut self.app,
                    &mut self.rec,
                    None,
                    e,
                );
                if let Some(m) = msg {
                    next.extend(self.app.update(m));
                }
            }
            fx = next;
        }
    }

    /// Turns syntax highlighting on (it is off by default in tests).
    pub fn with_syntax(mut self) -> Self {
        self.app.config.syntax = true;
        self
    }

    pub fn send(&mut self, msg: Msg) {
        let fx = self.app.update(msg);
        self.run(fx);
    }

    /// Space-separated keys: `j`, `enter`, `tab`, `esc`, `space`, `ctrl-d`…
    pub fn keys(&mut self, spec: &str) {
        for k in key_events(spec) {
            self.send(Msg::Key(k));
        }
    }

    /// Types text, spaces included, one key per character.
    pub fn type_text(&mut self, text: &str) {
        for k in text_events(text) {
            self.send(Msg::Key(k));
        }
    }

    pub fn refresh(&mut self, kind: RefreshKind) {
        self.send(Msg::Fs(kind));
    }

    pub fn render(&mut self, w: u16, h: u16) -> String {
        buffer_text(&self.render_buffer(w, h))
    }

    /// The rendered cells, for checking colors.
    pub fn render_buffer(&mut self, w: u16, h: u16) -> ratatui::buffer::Buffer {
        self.send(Msg::Resize(w, h));
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| spotter::ui::draw(f, &mut self.app)).unwrap();
        // Drawing can request highlighting of newly visible files.
        t.backend().buffer().clone()
    }
}

/// An App and its worker for `repo`, set up like main does, with a fixed
/// clock and highlighting off.
pub fn test_app(
    repo: &Repo,
    marks: Marks,
    config: Option<&Path>,
    f: impl FnOnce(&mut Config),
) -> (App, Worker) {
    let loaded = Config::load(config, &repo.git);
    let mut cfg = loaded.config;
    // Highlighting is opt-in per test (`with_syntax`), for speed.
    cfg.syntax = false;
    f(&mut cfg);
    let app = App::new(
        repo.empty_tree.clone(),
        marks,
        WatchStatus::Live,
        cfg.clone(),
    )
    .with_clock(fixed_now)
    .with_settings(
        loaded.sources,
        config.map(Path::to_path_buf),
        Some("truecolor".into()),
        None,
    );
    let worker = Worker::new(repo.clone(), cfg, None);
    (app, worker)
}

// ---------------------------------------------------------------------------
// Driving several repositories in tabs.

use spotter::msg::TabId;
use spotter::workspace::Workspace;
use spotter::workspace::discover::{self, Limits};

pub struct WsHarness {
    pub ws: Workspace,
    pub runtimes: Vec<(Worker, Repo)>,
    pub rec: Recorded,
    /// The settings file all tabs share.
    pub config: PathBuf,
}

impl WsHarness {
    /// Opens `paths` like `spotter <paths>` does, in `r`'s isolated config.
    pub fn open(r: &TestRepo, paths: &[&Path]) -> Self {
        let paths: Vec<PathBuf> = paths.iter().map(|p| p.to_path_buf()).collect();
        let opened = discover::open_with_env(&paths, &Limits::default(), r.env())
            .expect("open repositories");
        let config = r.tmp.path().join("config.toml");
        let mut runtimes = Vec::new();
        let mut tabs = Vec::new();
        for found in opened.repos {
            let (app, worker) = test_app(&found.repo, Marks::in_memory(), Some(&config), |_| {});
            tabs.push((found.name, app));
            runtimes.push((worker, found.repo));
        }
        let title = opened
            .folder
            .as_ref()
            .and_then(|f| f.file_name())
            .map(|n| n.to_string_lossy().into_owned());
        let mut h = WsHarness {
            ws: Workspace::new(tabs, title),
            runtimes,
            rec: Recorded::default(),
            config,
        };
        if let Some(n) = opened.notes.first() {
            h.ws.toast(n.clone());
        }
        let fx = h.ws.start();
        h.run(fx);
        h
    }

    pub fn run(&mut self, mut fx: Vec<(TabId, Effect)>) {
        while !fx.is_empty() {
            let mut next = Vec::new();
            for (id, e) in fx {
                let (worker, repo) = &mut self.runtimes[id.0 as usize];
                let app = self.ws.app_mut(id).expect("tab");
                if let Some(m) = execute(worker, repo, app, &mut self.rec, Some(&self.config), e) {
                    next.extend(self.ws.update(Msg::Tab(id, Box::new(m))));
                }
            }
            fx = next;
        }
    }

    pub fn send(&mut self, msg: Msg) {
        let fx = self.ws.update(msg);
        self.run(fx);
    }

    /// A message from tab `i`'s threads (a commit result, a prompt…).
    pub fn send_to(&mut self, i: u32, msg: Msg) {
        self.send(Msg::Tab(TabId(i), Box::new(msg)));
    }

    pub fn keys(&mut self, spec: &str) {
        for k in key_events(spec) {
            self.send(Msg::Key(k));
        }
    }

    pub fn type_text(&mut self, text: &str) {
        for k in text_events(text) {
            self.send(Msg::Key(k));
        }
    }

    /// Tab `i` notices its files changed.
    pub fn refresh(&mut self, i: u32, kind: RefreshKind) {
        self.send_to(i, Msg::Fs(kind));
    }

    pub fn app(&self, i: usize) -> &App {
        &self.ws.tabs[i].app
    }

    pub fn render(&mut self, w: u16, h: u16) -> String {
        buffer_text(&self.render_buffer(w, h))
    }

    pub fn render_buffer(&mut self, w: u16, h: u16) -> ratatui::buffer::Buffer {
        self.send(Msg::Resize(w, h));
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| spotter::ui::draw_workspace(f, &mut self.ws))
            .unwrap();
        t.backend().buffer().clone()
    }
}

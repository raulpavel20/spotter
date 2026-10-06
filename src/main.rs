use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::Parser;
use crossterm::event::{self, Event};

use spotter::app::App;
use spotter::askpass;
use spotter::config::{self, Args, Config};
use spotter::config_file;
use spotter::editor::{self, Prepared};
use spotter::git::Repo;
use spotter::git::write;
use spotter::highlight::{self, HlMsg, HlRequest};
use spotter::msg::{Effect, Msg, Outbox, RefreshKind, TabId, WatchStatus};
use spotter::review::{self, Marks};
use spotter::worker::{self, Request, Worker};
use spotter::workspace::Workspace;
use spotter::workspace::discover::{self, Limits, Opened};
use spotter::{term, ui, watch};

const TICK: Duration = Duration::from_millis(250);

fn main() -> ExitCode {
    // Started by git or ssh as the password helper for a commit or push.
    if let Some(socket) = std::env::var_os(askpass::ENV) {
        let prompt: Vec<String> = std::env::args().skip(1).collect();
        let confirm = std::env::var("SSH_ASKPASS_PROMPT").is_ok_and(|v| v == "confirm");
        return ExitCode::from(askpass::client(
            Path::new(&socket),
            &prompt.join(" "),
            confirm,
        ));
    }
    let args = Args::parse();
    if let Some(path) = spotter::log::init_from_env() {
        spotter::log::line(|| {
            format!(
                "spotter {} started; logging to {path}",
                env!("CARGO_PKG_VERSION")
            )
        });
    }
    let paths = match args.paths.as_slice() {
        [] => vec![PathBuf::from(".")],
        p => p.to_vec(),
    };
    let opened = match discover::open(&paths, &Limits::default()) {
        Ok(o) => o,
        Err(e) => {
            spotter::log::line(|| format!("discovery failed: {e}"));
            eprintln!("spotter: {e}");
            if let Some(hint) = e.hint() {
                eprintln!("{hint}");
            }
            return ExitCode::from(2);
        }
    };
    for f in &opened.repos {
        let repo = &f.repo;
        spotter::log::line(|| {
            format!(
                "repo {}: {} (git dir {}, common dir {}), git {}",
                f.name,
                repo.root.display(),
                repo.git_dir.display(),
                repo.common_dir.display(),
                repo.version
            )
        });
    }
    for n in &opened.notes {
        spotter::log::line(|| format!("note: {n}"));
    }
    match run(opened, args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            term::restore();
            eprintln!("spotter: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// Reads terminal events on a thread; can be parked while an editor owns
/// the terminal.
struct Input {
    paused: Arc<AtomicBool>,
    parked: Arc<AtomicBool>,
}

impl Input {
    fn spawn(tx: Sender<Msg>) -> Input {
        let paused = Arc::new(AtomicBool::new(false));
        let parked = Arc::new(AtomicBool::new(false));
        let (p, k) = (paused.clone(), parked.clone());
        thread::Builder::new()
            .name("spotter-input".into())
            .spawn(move || {
                loop {
                    if p.load(Ordering::SeqCst) {
                        k.store(true, Ordering::SeqCst);
                        thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    k.store(false, Ordering::SeqCst);
                    match event::poll(Duration::from_millis(50)) {
                        Ok(true) => {}
                        Ok(false) => continue,
                        Err(_) => return,
                    }
                    let msg = match event::read() {
                        Ok(Event::Key(k)) => Msg::Key(k),
                        Ok(Event::Resize(w, h)) => Msg::Resize(w, h),
                        Ok(Event::Paste(s)) => Msg::Paste(s),
                        Ok(_) => continue,
                        Err(_) => return,
                    };
                    if tx.send(msg).is_err() {
                        return;
                    }
                }
            })
            .expect("spawn input thread");
        Input { paused, parked }
    }

    fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
        let start = Instant::now();
        while !self.parked.load(Ordering::SeqCst) && start.elapsed() < Duration::from_millis(500) {
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }
}

fn mtime(path: Option<&std::path::Path>) -> Option<std::time::SystemTime> {
    std::fs::metadata(path?).and_then(|m| m.modified()).ok()
}

/// Runs an editor: GUI ones in the background, terminal ones in the
/// foreground with the TUI suspended.
fn launch(
    l: &editor::Launch,
    cwd: &std::path::Path,
    input: &Input,
    terminal: &mut term::Tui,
) -> anyhow::Result<Result<(), String>> {
    spotter::log::line(|| format!("editor: {:?} (gui: {})", l.argv, l.gui));
    if l.gui {
        return Ok(editor::spawn_detached(l, cwd));
    }
    input.pause();
    term::suspend();
    let r = editor::run_foreground(l, cwd);
    term::resume(terminal)?;
    input.resume();
    Ok(r)
}

/// The environment for git writes: prompts go to the password panel. The
/// helper's socket opens on the first write.
fn write_env(server: &mut Option<askpass::Server>, out: &Outbox) -> Vec<(OsString, OsString)> {
    if server.is_none() {
        match askpass::Server::start(out.clone()) {
            Ok(s) => *server = Some(s),
            Err(e) => spotter::log::line(|| format!("askpass: could not listen: {e}")),
        }
    }
    match (server.as_ref(), std::env::current_exe()) {
        (Some(s), Ok(exe)) => askpass::env(&exe, s.socket()),
        _ => Vec::new(),
    }
}

/// Runs git's editor on the commit message, with the TUI suspended, and
/// returns the edited text.
fn edit_message(
    repo: &Repo,
    text: &str,
    input: &Input,
    terminal: &mut term::Tui,
) -> anyhow::Result<Result<String, String>> {
    let editor = match write::editor(repo) {
        Ok(e) => e,
        Err(e) => return Ok(Err(e)),
    };
    let file = repo.git_dir.join("SPOTTER_EDITMSG");
    if let Err(e) = std::fs::write(&file, text) {
        return Ok(Err(format!("writing {}: {e}", file.display())));
    }
    spotter::log::line(|| format!("message editor: {editor}"));
    input.pause();
    term::suspend();
    // Like git: the editor setting is a shell snippet.
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$@\""))
        .arg(&editor)
        .arg(&file)
        .current_dir(&repo.root)
        .status();
    term::resume(terminal)?;
    input.resume();
    let result = match status {
        Ok(s) if s.success() => std::fs::read_to_string(&file).map_err(|e| e.to_string()),
        Ok(s) => Err(format!("{editor} exited with {s}")),
        Err(e) => Err(format!("could not run {editor}: {e}")),
    };
    let _ = std::fs::remove_file(&file);
    Ok(result)
}

/// One repository's side of things: its worker, and where its threads
/// send messages.
struct Runtime {
    repo: Repo,
    worker: Sender<Request>,
    out: Outbox,
    askpass: Option<askpass::Server>,
}

/// Watches on a thread of its own: walking a big tree takes a moment, and
/// several repositories would hold up the first frame.
fn start_watcher(repo: &Repo, out: Outbox) {
    let repo = repo.clone();
    thread::spawn(move || match watch::spawn(&repo, out.clone()) {
        // The watcher lives on its own thread; changes made while it was
        // being set up are caught by one more look.
        Ok(_) => {
            let _ = out.send(Msg::Fs(RefreshKind::Worktree));
        }
        Err(e) => {
            spotter::log::line(|| {
                format!(
                    "watcher for {} failed, polling instead: {e}",
                    repo.root.display()
                )
            });
            let _ = out.send(Msg::Watch(WatchStatus::Error(e)));
        }
    });
}

fn run(opened: Opened, args: Args) -> anyhow::Result<()> {
    let config_path = args.config.clone().or_else(config::default_path);
    let (tx, rx) = mpsc::channel::<Msg>();
    let env = |k: &str| std::env::var(k).ok();
    // Several repositories share the patch cache's memory.
    let budget =
        (256 * 1024 * 1024 / opened.repos.len()).clamp(8 * 1024 * 1024, worker::PATCH_BUDGET);
    let mut runtimes = Vec::new();
    let mut tabs = Vec::new();
    for (i, found) in opened.repos.into_iter().enumerate() {
        let repo = found.repo;
        let out = Outbox::tagged(tx.clone(), TabId(i as u32));
        let loaded = Config::load(config_path.as_deref(), &repo.git);
        let cfg = loaded.config.clone();
        let w = Worker::new(repo.clone(), cfg.clone(), args.base.clone()).with_patch_budget(budget);
        let worker = worker::spawn(w, out.clone());
        let status = if args.no_watch {
            WatchStatus::Polling
        } else {
            start_watcher(&repo, out.clone());
            WatchStatus::Live
        };
        let mut app = App::new(
            repo.empty_tree.clone(),
            Marks::load(repo.marks_path()),
            status,
            cfg,
        )
        .with_settings(
            loaded.sources.clone(),
            config_path.clone(),
            env("COLORTERM"),
            env("COLORFGBG"),
        );
        if i == 0 {
            spotter::log::line(|| {
                format!(
                    "settings from {}; warnings: {:?}",
                    config_path
                        .as_ref()
                        .map_or("(none)".into(), |p| p.display().to_string()),
                    loaded.warnings
                )
            });
        }
        if let Some(w) = loaded.warnings.first() {
            app.error = Some(format!("settings: {w}"));
        }
        tabs.push((found.name, app));
        runtimes.push(Runtime {
            repo,
            worker,
            out,
            askpass: None,
        });
    }
    let title = opened
        .folder
        .as_ref()
        .and_then(|f| f.file_name())
        .map(|n| n.to_string_lossy().into_owned());
    let mut ws = Workspace::new(tabs, title);
    if let Some(note) = opened.notes.first() {
        ws.toast(note.clone());
    }
    // Always running, so syntax highlighting can be switched on live.
    let highlighter = highlight::spawn(ws.syntax_theme(), tx.clone());
    let mut config_mtime = mtime(config_path.as_deref());

    let mut terminal = term::init().context("terminal setup")?;
    if std::env::var_os("SPOTTER_TEST_PANIC").is_some() {
        panic!("forced panic (SPOTTER_TEST_PANIC)");
    }
    let size = terminal.size()?;
    let input = Input::spawn(tx.clone());

    let mut effects = ws.update(Msg::Resize(size.width, size.height));
    effects.extend(ws.start());
    let mut last_tick = Instant::now();
    let mut dirty = true;
    loop {
        // Execute effects; some produce follow-up effects.
        while !effects.is_empty() {
            let mut next = Vec::new();
            for (id, fx) in effects.drain(..) {
                let Some(rt) = runtimes.get_mut(id.0 as usize) else {
                    continue;
                };
                // Something for this tab's screen to say.
                let say = |ws: &mut Workspace, text: String| {
                    if let Some(app) = ws.app_mut(id) {
                        app.toast = Some((text, spotter::app::TOAST_TICKS));
                    }
                };
                let fail = |ws: &mut Workspace, text: String| {
                    if let Some(app) = ws.app_mut(id) {
                        app.error = Some(text);
                    }
                };
                match fx {
                    Effect::Quit => {
                        term::restore();
                        return Ok(());
                    }
                    Effect::Refresh { seq, kind, opts } => {
                        let _ = rt.worker.send(Request::Refresh { seq, kind, opts });
                    }
                    Effect::LoadPatch {
                        seq,
                        spec,
                        files,
                        opts,
                    } => {
                        let _ = rt.worker.send(Request::Patch {
                            seq,
                            spec,
                            files,
                            opts,
                        });
                    }
                    Effect::LoadFile {
                        seq,
                        index,
                        spec,
                        file,
                        opts,
                    } => {
                        let _ = rt.worker.send(Request::File {
                            seq,
                            index,
                            spec,
                            file,
                            opts,
                        });
                    }
                    Effect::Highlight {
                        seq,
                        index,
                        key,
                        path,
                        patch,
                    } => {
                        let _ = highlighter.send(HlMsg::Highlight(HlRequest {
                            tab: Some(id),
                            seq,
                            index,
                            key,
                            path,
                            patch,
                        }));
                    }
                    Effect::SaveSetting { key, value } => match &config_path {
                        Some(p) => match config_file::save(p, key, value.as_ref()) {
                            Ok(()) => config_mtime = mtime(Some(p)),
                            Err(e) => fail(&mut ws, format!("saving settings: {e}")),
                        },
                        None => fail(&mut ws, "no settings file: set HOME or use --config".into()),
                    },
                    Effect::ReloadConfig => {
                        let loaded = Config::load(config_path.as_deref(), &rt.repo.git);
                        next.extend(ws.update(Msg::Tab(
                            id,
                            Box::new(Msg::ConfigReloaded(Box::new(loaded))),
                        )));
                    }
                    Effect::SetWorkerConfig(c) => {
                        let _ = rt.worker.send(Request::SetConfig(*c));
                    }
                    Effect::SetSyntaxTheme(t) => {
                        let _ = highlighter.send(HlMsg::SetTheme(t));
                    }
                    Effect::EditConfig => {
                        let Some(p) = &config_path else {
                            fail(&mut ws, "no settings file: set HOME or use --config".into());
                            continue;
                        };
                        if let Err(e) = config_file::ensure_exists(p) {
                            fail(&mut ws, format!("creating {}: {e}", p.display()));
                            continue;
                        }
                        let Some(app) = ws.app(id) else { continue };
                        let l = editor::resolve(
                            &app.config,
                            std::env::var("VISUAL").ok(),
                            std::env::var("EDITOR").ok(),
                            &p.to_string_lossy(),
                            1,
                        );
                        if let Err(e) = launch(&l, &rt.repo.root, &input, &mut terminal)? {
                            say(&mut ws, e);
                        }
                        // The file is re-read on the next tick if it changed.
                    }
                    Effect::SaveMarks => {
                        let saved = ws.app_mut(id).map(|a| a.marks.save(review::now()));
                        if let Some(Err(e)) = saved {
                            fail(&mut ws, format!("saving viewed marks: {e}"));
                        }
                    }
                    Effect::Redraw => term::reset(&mut terminal)?,
                    Effect::Commit { files, message } => {
                        let env = write_env(&mut rt.askpass, &rt.out);
                        if write::signs_with_gpg(&rt.repo) {
                            // gpg's pinentry needs the terminal.
                            input.pause();
                            term::suspend();
                            println!("spotter: committing; gpg may ask for your passphrase…");
                            let result = write::commit(&rt.repo, &files, &message, &env);
                            term::resume(&mut terminal)?;
                            input.resume();
                            next.extend(ws.update(Msg::Tab(id, Box::new(Msg::Committed(result)))));
                        } else {
                            let (repo, out) = (rt.repo.clone(), rt.out.clone());
                            thread::spawn(move || {
                                let result = write::commit(&repo, &files, &message, &env);
                                let _ = out.send(Msg::Committed(result));
                            });
                        }
                    }
                    Effect::PreparePush => {
                        let (repo, out) = (rt.repo.clone(), rt.out.clone());
                        thread::spawn(move || {
                            let _ = out.send(Msg::PushInfo(write::push_info(&repo)));
                        });
                    }
                    Effect::Push {
                        remote,
                        branch,
                        dest,
                        set_upstream,
                    } => {
                        let env = write_env(&mut rt.askpass, &rt.out);
                        let (repo, out) = (rt.repo.clone(), rt.out.clone());
                        thread::spawn(move || {
                            let result =
                                write::push(&repo, &remote, &branch, &dest, set_upstream, &env);
                            let _ = out.send(Msg::Pushed(result));
                        });
                    }
                    Effect::EditMessage(text) => {
                        match edit_message(&rt.repo, &text, &input, &mut terminal)? {
                            Ok(edited) => next.extend(
                                ws.update(Msg::Tab(id, Box::new(Msg::MessageEdited(edited)))),
                            ),
                            Err(e) => say(&mut ws, e),
                        }
                    }
                    Effect::Answer { id: prompt, answer } => {
                        if let Some(s) = &rt.askpass {
                            s.answer(prompt, answer.map(|a| a.0));
                        }
                    }
                    Effect::OpenEditor(req) => {
                        let Some(app) = ws.app(id) else { continue };
                        match editor::prepare(&rt.repo, &app.config, &req) {
                            Prepared::Refused(msg) => say(&mut ws, msg),
                            Prepared::Run { launch: l, note } => {
                                let result = launch(&l, &rt.repo.root, &input, &mut terminal)?;
                                let msg = match result {
                                    Ok(()) => note,
                                    Err(e) => Some(e),
                                };
                                if let Some(m) = msg {
                                    say(&mut ws, m);
                                }
                                next.extend(ws.update(Msg::Tab(
                                    id,
                                    Box::new(Msg::Fs(RefreshKind::Worktree)),
                                )));
                            }
                        }
                    }
                }
            }
            effects = next;
            dirty = true;
        }

        if dirty {
            terminal.draw(|f| ui::draw_workspace(f, &mut ws))?;
            dirty = false;
        }

        let timeout = TICK.saturating_sub(last_tick.elapsed());
        match rx.recv_timeout(timeout) {
            Ok(msg) => {
                effects.extend(ws.update(msg));
                for msg in rx.try_iter() {
                    effects.extend(ws.update(msg));
                }
                dirty = true;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
        if last_tick.elapsed() >= TICK {
            last_tick = Instant::now();
            let had_toast = ws.active_app().toast.is_some();
            effects.extend(ws.update(Msg::Tick));
            // Settings edited outside the screen (or with `e`).
            let m = mtime(config_path.as_deref());
            if m != config_mtime {
                config_mtime = m;
                for (i, rt) in runtimes.iter().enumerate() {
                    let loaded = Config::load(config_path.as_deref(), &rt.repo.git);
                    let msg = Msg::ConfigReloaded(Box::new(loaded));
                    effects.extend(ws.update(Msg::Tab(TabId(i as u32), Box::new(msg))));
                }
                dirty = true;
            }
            // Another instance changed marks: viewed files fold.
            if ws.reload_marks() {
                dirty = true;
            }
            if had_toast != ws.active_app().toast.is_some() {
                dirty = true;
            }
        }
    }
}

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
use spotter::config::{self, Args, Config};
use spotter::config_file;
use spotter::editor::{self, Prepared};
use spotter::git::Repo;
use spotter::highlight::{self, HlMsg, HlRequest};
use spotter::msg::{Effect, Msg, RefreshKind, WatchStatus};
use spotter::review::{self, Marks};
use spotter::worker::{self, Request};
use spotter::{term, ui, watch};

const TICK: Duration = Duration::from_millis(250);

fn main() -> ExitCode {
    let args = Args::parse();
    if let Some(path) = spotter::log::init_from_env() {
        spotter::log::line(|| {
            format!(
                "spotter {} started; logging to {path}",
                env!("CARGO_PKG_VERSION")
            )
        });
    }
    let path = args.path.clone().unwrap_or_else(|| ".".into());
    let repo = match Repo::discover(&path) {
        Ok(r) => r,
        Err(e) => {
            spotter::log::line(|| format!("discovery failed: {e}"));
            eprintln!("spotter: {e}");
            return ExitCode::from(2);
        }
    };
    spotter::log::line(|| {
        format!(
            "repo {} (git dir {}, common dir {}), git {}",
            repo.root.display(),
            repo.git_dir.display(),
            repo.common_dir.display(),
            repo.version
        )
    });
    match run(repo, args) {
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

fn run(repo: Repo, args: Args) -> anyhow::Result<()> {
    let config_path = args.config.clone().or_else(config::default_path);
    let loaded = Config::load(config_path.as_deref(), &repo.git);
    let cfg = loaded.config.clone();
    let (tx, rx) = mpsc::channel::<Msg>();
    let worker = worker::spawn(repo.clone(), cfg.clone(), args.base.clone(), tx.clone());
    let (status, _watcher) = if args.no_watch {
        (WatchStatus::Polling, None)
    } else {
        match watch::spawn(&repo, tx.clone()) {
            Ok(h) => (WatchStatus::Live, Some(h)),
            Err(e) => {
                spotter::log::line(|| format!("watcher failed, polling instead: {e}"));
                (WatchStatus::Error(e), None)
            }
        }
    };
    let env = |k: &str| std::env::var(k).ok();
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
    spotter::log::line(|| {
        format!(
            "settings from {}; warnings: {:?}",
            config_path
                .as_ref()
                .map_or("(none)".into(), |p| p.display().to_string()),
            loaded.warnings
        )
    });
    if let Some(w) = loaded.warnings.first() {
        app.error = Some(format!("settings: {w}"));
    }
    // Always running, so syntax highlighting can be switched on live.
    let highlighter = highlight::spawn(app.syntax_theme(), tx.clone());
    let mut config_mtime = mtime(config_path.as_deref());

    let mut terminal = term::init().context("terminal setup")?;
    if std::env::var_os("SPOTTER_TEST_PANIC").is_some() {
        panic!("forced panic (SPOTTER_TEST_PANIC)");
    }
    let size = terminal.size()?;
    app.size = (size.width, size.height);
    let input = Input::spawn(tx.clone());

    let mut effects = app.start();
    let mut last_tick = Instant::now();
    let mut dirty = true;
    loop {
        // Execute effects; some produce follow-up effects.
        while !effects.is_empty() {
            let mut next = Vec::new();
            for fx in effects.drain(..) {
                match fx {
                    Effect::Quit => {
                        term::restore();
                        return Ok(());
                    }
                    Effect::Refresh { seq, kind, opts } => {
                        let _ = worker.send(Request::Refresh { seq, kind, opts });
                    }
                    Effect::LoadPatch {
                        seq,
                        spec,
                        files,
                        opts,
                    } => {
                        let _ = worker.send(Request::Patch {
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
                        let _ = worker.send(Request::File {
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
                            Err(e) => app.error = Some(format!("saving settings: {e}")),
                        },
                        None => {
                            app.error = Some("no settings file: set HOME or use --config".into())
                        }
                    },
                    Effect::SetWorkerConfig(c) => {
                        let _ = worker.send(Request::SetConfig(*c));
                    }
                    Effect::SetSyntaxTheme(t) => {
                        let _ = highlighter.send(HlMsg::SetTheme(t));
                    }
                    Effect::EditConfig => {
                        let Some(p) = &config_path else {
                            app.error = Some("no settings file: set HOME or use --config".into());
                            continue;
                        };
                        if let Err(e) = config_file::ensure_exists(p) {
                            app.error = Some(format!("creating {}: {e}", p.display()));
                            continue;
                        }
                        let l = editor::resolve(
                            &app.config,
                            std::env::var("VISUAL").ok(),
                            std::env::var("EDITOR").ok(),
                            &p.to_string_lossy(),
                            1,
                        );
                        if let Err(e) = launch(&l, &repo.root, &input, &mut terminal)? {
                            app.toast = Some((e, spotter::app::TOAST_TICKS));
                        }
                        // The file is re-read on the next tick if it changed.
                    }
                    Effect::SaveMarks => {
                        if let Err(e) = app.marks.save(review::now()) {
                            app.error = Some(format!("saving viewed marks: {e}"));
                        }
                    }
                    Effect::Redraw => term::reset(&mut terminal)?,
                    Effect::OpenEditor(req) => match editor::prepare(&repo, &app.config, &req) {
                        Prepared::Refused(msg) => {
                            app.toast = Some((msg, spotter::app::TOAST_TICKS))
                        }
                        Prepared::Run { launch: l, note } => {
                            let result = launch(&l, &repo.root, &input, &mut terminal)?;
                            let msg = match result {
                                Ok(()) => note,
                                Err(e) => Some(e),
                            };
                            if let Some(m) = msg {
                                app.toast = Some((m, spotter::app::TOAST_TICKS));
                            }
                            next.extend(app.update(Msg::Fs(RefreshKind::Worktree)));
                        }
                    },
                }
            }
            effects = next;
            dirty = true;
        }

        if dirty {
            terminal.draw(|f| ui::draw(f, &mut app))?;
            dirty = false;
        }

        let timeout = TICK.saturating_sub(last_tick.elapsed());
        match rx.recv_timeout(timeout) {
            Ok(msg) => {
                effects.extend(app.update(msg));
                for msg in rx.try_iter() {
                    effects.extend(app.update(msg));
                }
                dirty = true;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
        if last_tick.elapsed() >= TICK {
            last_tick = Instant::now();
            let had_toast = app.toast.is_some();
            effects.extend(app.update(Msg::Tick));
            // Settings edited outside the screen (or with `e`).
            let m = mtime(config_path.as_deref());
            if m != config_mtime {
                config_mtime = m;
                let loaded = Config::load(config_path.as_deref(), &repo.git);
                effects.extend(app.update(Msg::ConfigReloaded(Box::new(loaded))));
                dirty = true;
            }
            if app.marks.reload_if_changed() {
                // Another instance changed marks: viewed files fold.
                app.sync_viewed();
                dirty = true;
            }
            if had_toast != app.toast.is_some() {
                dirty = true;
            }
        }
    }
}

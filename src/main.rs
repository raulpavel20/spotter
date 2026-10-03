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
use spotter::config::{Args, Config};
use spotter::editor::{self, Prepared};
use spotter::git::Repo;
use spotter::msg::{Effect, Msg, RefreshKind, WatchStatus};
use spotter::review::{self, Marks};
use spotter::worker::{self, Request};
use spotter::{term, ui, watch};

const TICK: Duration = Duration::from_millis(250);

fn main() -> ExitCode {
    let args = Args::parse();
    let path = args.path.clone().unwrap_or_else(|| ".".into());
    let repo = match Repo::discover(&path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("spotter: {e}");
            return ExitCode::from(2);
        }
    };
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

fn run(repo: Repo, args: Args) -> anyhow::Result<()> {
    let cfg = Config::load(&repo.git);
    let (tx, rx) = mpsc::channel::<Msg>();
    let worker = worker::spawn(repo.clone(), cfg.clone(), args.base.clone(), tx.clone());
    let (status, _watcher) = if args.no_watch {
        (WatchStatus::Polling, None)
    } else {
        match watch::spawn(&repo, tx.clone()) {
            Ok(h) => (WatchStatus::Live, Some(h)),
            Err(e) => (WatchStatus::Error(e), None),
        }
    };
    let mut app = App::new(
        repo.empty_tree.clone(),
        Marks::load(repo.marks_path()),
        status,
    );
    app.tab_width = cfg.tab_width;

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
                    Effect::LoadPatch { seq, spec, files } => {
                        let _ = worker.send(Request::Patch { seq, spec, files });
                    }
                    Effect::LoadFile {
                        seq,
                        index,
                        spec,
                        file,
                    } => {
                        let _ = worker.send(Request::File {
                            seq,
                            index,
                            spec,
                            file,
                        });
                    }
                    Effect::SaveMarks => {
                        if let Err(e) = app.marks.save(review::now()) {
                            app.error = Some(format!("saving viewed marks: {e}"));
                        }
                    }
                    Effect::Redraw => term::reset(&mut terminal)?,
                    Effect::OpenEditor(req) => match editor::prepare(&repo, &cfg, &req) {
                        Prepared::Refused(msg) => {
                            app.toast = Some((msg, spotter::app::TOAST_TICKS))
                        }
                        Prepared::Run { launch, note } => {
                            let result = if launch.gui {
                                editor::spawn_detached(&launch, &repo.root)
                            } else {
                                input.pause();
                                term::suspend();
                                let r = editor::run_foreground(&launch, &repo.root);
                                term::resume(&mut terminal)?;
                                input.resume();
                                r
                            };
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
            if app.marks.reload_if_changed() || had_toast != app.toast.is_some() {
                dirty = true;
            }
        }
    }
}

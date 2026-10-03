//! Terminal setup, teardown and suspension.
//!
//! The panic hook restores the terminal before printing the panic, then
//! exits, so a panic on any thread never leaves the shell in raw mode.

use std::io::{self, Stdout, stdout};
use std::sync::Once;

use crossterm::cursor::Show;
use crossterm::execute;
use crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

pub type Tui = Terminal<CrosstermBackend<Stdout>>;

static HOOK: Once = Once::new();

fn install_panic_hook() {
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            previous(info);
            std::process::exit(101);
        }));
    });
}

pub fn init() -> io::Result<Tui> {
    install_panic_hook();
    enable()?;
    Terminal::new(CrosstermBackend::new(stdout()))
}

fn enable() -> io::Result<()> {
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)
}

/// Best-effort restore; safe to call more than once.
pub fn restore() {
    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen, Show);
}

/// Hand the terminal to a child process (an editor).
pub fn suspend() {
    restore();
}

/// Take the terminal back and force a full redraw.
pub fn resume(terminal: &mut Tui) -> io::Result<()> {
    enable()?;
    reset(terminal)
}

/// Clears the screen and starts from a fresh `Terminal`, so the next draw
/// repaints everything. Unlike `Terminal::clear`, this never queries the
/// cursor position, which some terminals answer slowly or not at all.
pub fn reset(terminal: &mut Tui) -> io::Result<()> {
    execute!(stdout(), Clear(ClearType::All))?;
    *terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    Ok(())
}

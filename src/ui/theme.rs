//! Styles. Only the 16 basic colors and modifiers, so light and dark
//! terminal themes both work.

use ratatui::style::{Color, Modifier, Style};

use crate::app::Glyph;
use crate::model::Status;

pub fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

pub fn bold() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

pub fn add() -> Style {
    Style::new().fg(Color::Green)
}

pub fn del() -> Style {
    Style::new().fg(Color::Red)
}

pub fn hunk() -> Style {
    Style::new().fg(Color::Cyan)
}

pub fn sha() -> Style {
    Style::new().fg(Color::Yellow)
}

pub fn banner() -> Style {
    Style::new().fg(Color::Yellow)
}

pub fn error() -> Style {
    Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)
}

pub fn toast() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

/// The cursor row of the focused panel.
pub fn cursor() -> Style {
    Style::new().add_modifier(Modifier::REVERSED)
}

pub fn border(focused: bool) -> Style {
    if focused { Style::new() } else { dim() }
}

pub fn glyph(g: Glyph) -> Style {
    match g {
        Glyph::All => Style::new().fg(Color::Green),
        Glyph::Some => Style::new().fg(Color::Yellow),
        Glyph::None => Style::new().fg(Color::Blue),
    }
}

pub fn viewed() -> Style {
    Style::new().fg(Color::Green)
}

pub fn status(s: Status) -> Style {
    match s {
        Status::Added | Status::Untracked => Style::new().fg(Color::Green),
        Status::Deleted => Style::new().fg(Color::Red),
        Status::Renamed(_) | Status::Copied(_) => Style::new().fg(Color::Cyan),
        Status::Unmerged => Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        Status::Modified | Status::TypeChange => Style::new().fg(Color::Yellow),
        Status::Unknown => Style::new(),
    }
}

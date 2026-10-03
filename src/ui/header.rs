//! Header, banner and footer lines.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::text::left_right;
use super::theme;
use crate::app::{App, Focus};
use crate::git::base::BaseMode;
use crate::msg::WatchStatus;

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let wide = area.width >= super::WIDE;
    let mut left: Vec<Span<'static>> = vec![Span::raw(" ")];
    match &app.snap {
        None => left.push(Span::styled("loading…", theme::dim())),
        Some(s) => {
            let b = &s.base;
            match (&b.branch, &b.head) {
                (Some(br), _) => left.push(Span::styled(br.clone(), theme::bold())),
                (None, Some(h)) => left.push(Span::styled(
                    format!("detached @{}", short(h)),
                    theme::bold(),
                )),
                (None, None) => left.push(Span::styled("(no branch)", theme::bold())),
            }
            let sep = || Span::styled(" · ", theme::dim());
            match b.mode {
                BaseMode::Unborn => {
                    left.push(sep());
                    left.push(Span::raw("no commits yet"));
                }
                BaseMode::TrunkRecent | BaseMode::NoBase => {
                    left.push(sep());
                    left.push(Span::raw(format!(
                        "last {}",
                        plural(s.commits.len(), "commit")
                    )));
                }
                _ => {
                    if let Some(base) = &b.base_ref {
                        left.push(sep());
                        if wide {
                            left.push(Span::raw("base "));
                        }
                        left.push(Span::raw(base.clone()));
                        if wide {
                            if let Some(mb) = &b.merge_base {
                                left.push(Span::styled(format!(" @{}", short(mb)), theme::dim()));
                            }
                        }
                    }
                    if wide {
                        left.push(sep());
                        left.push(Span::raw(plural(b.count, "commit")));
                    }
                }
            }
        }
    }

    let mut right: Vec<Span<'static>> = Vec::new();
    match &app.watch {
        WatchStatus::Live => right.push(Span::styled("● live", theme::add())),
        WatchStatus::Polling => right.push(Span::styled("◌ polling", theme::banner())),
        WatchStatus::Error(_) => right.push(Span::styled("✕ watch error", theme::error())),
    }
    if app.snap.is_some() && !app.commits().is_empty() {
        right.push(Span::styled(" · ", theme::dim()));
        match app.to_review() {
            0 => right.push(Span::styled("all reviewed ✓", theme::viewed())),
            n => right.push(Span::raw(format!("{n} to review"))),
        }
    }
    right.push(Span::raw(" "));
    f.render_widget(
        Paragraph::new(left_right(left, right, area.width as usize)),
        area,
    );
}

/// Banner lines: in-progress operations, base notes, errors.
pub fn banner_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(s) = &app.snap {
        let mut parts: Vec<String> = s.ops.iter().map(|o| o.to_string()).collect();
        parts.extend(s.base.notes.iter().cloned());
        if !s.remerge && s.commits.iter().any(|c| c.is_merge()) {
            parts.push("git too old for remerge-diff · merges show first-parent diff".into());
        }
        if !parts.is_empty() {
            let text = format!(" {}", parts.join(" · "));
            lines.push(Line::styled(
                super::text::truncate_end(&text, width as usize),
                theme::banner(),
            ));
        }
    }
    if let Some(e) = &app.error {
        let text = format!(" ✕ {e}");
        lines.push(Line::styled(
            super::text::truncate_end(&text, width as usize),
            theme::error(),
        ));
    }
    if let WatchStatus::Error(e) = &app.watch {
        if lines.len() < 2 {
            let text = format!(" watch error: {e} · polling every 2 s");
            lines.push(Line::styled(
                super::text::truncate_end(&text, width as usize),
                theme::banner(),
            ));
        }
    }
    lines
}

pub fn draw_banner(f: &mut Frame, area: Rect, lines: Vec<Line<'static>>) {
    if area.height > 0 {
        f.render_widget(Paragraph::new(lines), area);
    }
}

fn hint_spans(hints: &[(&str, &str)]) -> Vec<Span<'static>> {
    let mut spans = vec![Span::raw(" ")];
    for (i, (k, what)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", theme::dim()));
        }
        spans.push(Span::styled(k.to_string(), theme::bold()));
        spans.push(Span::styled(format!(" {what}"), theme::dim()));
    }
    spans
}

pub fn draw_footer(f: &mut Frame, area: Rect, app: &App, wide: bool) {
    let hints: &[(&str, &str)] = match (app.focus, wide) {
        (Focus::Timeline, true) => &[
            ("enter", "files"),
            ("r", "viewed"),
            ("u", "next to review"),
            ("w/b", "jump"),
            ("?", "help"),
        ],
        (Focus::Timeline, false) => &[("enter", "files"), ("u", "next"), ("?", "help")],
        (Focus::Files, true) => &[
            ("enter", "open"),
            ("space", "viewed"),
            ("u", "next to review"),
            ("e", "edit"),
            ("?", "help"),
        ],
        (Focus::Files, false) => &[
            ("enter", "open"),
            ("space", "viewed"),
            ("u", "next"),
            ("?", "help"),
        ],
    };
    draw_footer_with(f, area, app, hints);
}

pub fn draw_footer_with(f: &mut Frame, area: Rect, app: &App, hints: &[(&str, &str)]) {
    let right = match &app.toast {
        Some((t, _)) => vec![Span::styled(t.clone(), theme::toast()), Span::raw(" ")],
        None => Vec::new(),
    };
    let line = left_right(hint_spans(hints), right, area.width as usize);
    f.render_widget(Paragraph::new(line).style(Style::new()), area);
}

//! Header, banner and footer lines.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::text::{self, left_right, width};
use super::theme;
use crate::app::{App, Focus};
use crate::git::base::BaseMode;
use crate::model::TargetId;
use crate::msg::WatchStatus;

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let wide = area.width >= app.config.wide_breakpoint;
    let gl = app.glyphs();
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
                        if wide && let Some(mb) = &b.merge_base {
                            left.push(Span::styled(format!(" @{}", short(mb)), theme::dim()));
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
        WatchStatus::Live => right.push(Span::styled(format!("{} live", gl.live), theme::add())),
        WatchStatus::Polling => right.push(Span::styled(
            format!("{} polling", gl.polling),
            theme::banner(),
        )),
        WatchStatus::Error(_) => right.push(Span::styled(
            format!("{} watch error", gl.error),
            theme::error(),
        )),
    }
    if app.snap.is_some() && !app.commits().is_empty() {
        right.push(Span::styled(" · ", theme::dim()));
        match app.to_review() {
            0 => right.push(Span::styled(
                format!("all reviewed {}", gl.viewed),
                theme::viewed(),
            )),
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
        let text = format!(" {} {e}", app.glyphs().error);
        lines.push(Line::styled(
            super::text::truncate_end(&text, width as usize),
            theme::error(),
        ));
    }
    if let WatchStatus::Error(e) = &app.watch
        && lines.len() < 2
    {
        let text = format!(" watch error: {e} · polling every 2 s");
        lines.push(Line::styled(
            super::text::truncate_end(&text, width as usize),
            theme::banner(),
        ));
    }
    lines
}

pub fn draw_banner(f: &mut Frame, area: Rect, lines: Vec<Line<'static>>) {
    if area.height > 0 {
        f.render_widget(Paragraph::new(lines), area);
    }
}

/// A key hint for the footer. Lower `prio` survives longer when the line
/// is too narrow: 1 = essential, 3 = nice to have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    pub key: &'static str,
    pub what: String,
    pub prio: u8,
}

pub fn hint(key: &'static str, what: impl Into<String>, prio: u8) -> Hint {
    Hint {
        key,
        what: what.into(),
        prio,
    }
}

fn hints_width(hints: &[Hint]) -> usize {
    let items: usize = hints
        .iter()
        .map(|h| width(h.key) + 1 + width(&h.what))
        .sum();
    1 + items + 3 * hints.len().saturating_sub(1)
}

/// Drops the least important hints (latest first among equals) until the
/// rest fit in `room` columns, keeping their order.
pub fn fit_hints(mut hints: Vec<Hint>, room: usize) -> Vec<Hint> {
    while hints.len() > 1 && hints_width(&hints) > room {
        let worst = hints
            .iter()
            .enumerate()
            .max_by_key(|(i, h)| (h.prio, *i))
            .map(|(i, _)| i)
            .unwrap_or(0);
        hints.remove(worst);
    }
    hints
}

pub fn hint_spans(hints: &[Hint]) -> Vec<Span<'static>> {
    let mut spans = vec![Span::raw(" ")];
    for (i, h) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", theme::dim()));
        }
        spans.push(Span::styled(h.key, theme::bold()));
        spans.push(Span::styled(format!(" {}", h.what), theme::dim()));
    }
    spans
}

/// Hints for the main screen, for what is selected.
fn main_hints(app: &App) -> Vec<Hint> {
    let mut h = Vec::new();
    match app.focus {
        Focus::Timeline => {
            h.push(hint("enter", "files", 1));
            h.push(hint("u", "next to review", 1));
            if !app.selected_files().is_empty() {
                let all = app.glyph(app.selected_files()) == crate::app::Glyph::All;
                h.push(hint("r", if all { "unview all" } else { "view all" }, 2));
            }
            match app.selected() {
                Some(TargetId::Total) => h.push(hint(
                    "i",
                    if app.opts.include_wt {
                        "committed only"
                    } else {
                        "add uncommitted"
                    },
                    2,
                )),
                Some(TargetId::Commit(sha))
                    if app
                        .snap
                        .as_ref()
                        .and_then(|s| s.commit(&sha))
                        .is_some_and(|c| c.is_merge()) =>
                {
                    h.push(hint("m", "merge diff mode", 2))
                }
                _ => {}
            }
            h.push(hint("w/b", "jump", 3));
        }
        Focus::Files => {
            h.push(hint("enter", "open", 1));
            if let Some(f) = app.selected_files().get(app.file_sel) {
                h.push(hint(
                    "space",
                    if app.is_viewed(f) { "unview" } else { "viewed" },
                    1,
                ));
            }
            h.push(hint("u", "next to review", 1));
            h.push(hint("e", "edit", 2));
            h.push(hint("esc", "timeline", 3));
        }
    }
    h.push(hint(",", "settings", 3));
    h.push(hint("?", "help", 1));
    h
}

pub fn draw_footer(f: &mut Frame, area: Rect, app: &App) {
    draw_footer_hints(f, area, app, main_hints(app));
}

/// The footer: hints on the left (as many as fit), a toast on the right.
pub fn draw_footer_hints(f: &mut Frame, area: Rect, app: &App, hints: Vec<Hint>) {
    let right = match &app.toast {
        Some((t, _)) => vec![Span::styled(t.clone(), theme::toast()), Span::raw(" ")],
        None => Vec::new(),
    };
    let room = (area.width as usize).saturating_sub(text::spans_width(&right) + 1);
    let hints = fit_hints(hints, room);
    let line = left_right(hint_spans(&hints), right, area.width as usize);
    f.render_widget(Paragraph::new(line).style(Style::new()), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fitting_drops_least_important_hints_first() {
        let hints = vec![
            hint("enter", "open", 1),
            hint("w/b", "jump", 3),
            hint("space", "viewed", 1),
            hint(",", "settings", 3),
            hint("e", "edit", 2),
        ];
        let all = fit_hints(hints.clone(), 200);
        assert_eq!(all.len(), 5);
        let keys = |h: &[Hint]| h.iter().map(|h| h.key).collect::<Vec<_>>();
        // " enter open · space viewed · e edit" is 36 columns.
        assert_eq!(keys(&fit_hints(hints.clone(), 36)), ["enter", "space", "e"]);
        assert_eq!(keys(&fit_hints(hints, 10)), ["enter"]);
    }
}

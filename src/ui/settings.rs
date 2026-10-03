//! The settings panel (`,`): a centered modal listing every setting, with
//! faint leaders from each label to its value. Changes apply live and are
//! saved to the config file as you go.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use super::files::keep_visible;
use super::header::{hint, hint_spans};
use super::text::{self, width};
use super::theme;
use crate::app::App;
use crate::config::{Config, Kind, SETTINGS, Source, Value};

/// Content width bounds; within them the panel fits its widest row.
const MIN_WIDTH: usize = 44;
const MAX_WIDTH: usize = 60;
/// Lines of description under the list.
const INFO: u16 = 3;

fn section_title(section: &str) -> &'static str {
    match section {
        "review" => "Review",
        "diff" => "Diff",
        "theme" => "Appearance",
        "layout" => "Layout",
        "editor" => "Editor",
        "repo" => "Repository",
        _ => "Other",
    }
}

/// Panel rows: section headings and blank lines (`None`), and settings
/// (index into `SETTINGS`).
fn rows() -> Vec<(Option<usize>, &'static str)> {
    let mut out = Vec::new();
    let mut section = "";
    for (i, def) in SETTINGS.iter().enumerate() {
        if def.section() != section {
            section = def.section();
            if !out.is_empty() {
                out.push((None, ""));
            }
            out.push((None, section_title(section)));
        }
        out.push((Some(i), ""));
    }
    out
}

fn value_text(app: &App, i: usize) -> String {
    let def = &SETTINGS[i];
    let v = app.config.get(def.key);
    match (def.kind, &v) {
        (Kind::Theme, Value::Text(t)) if t == "default" => {
            format!("default ({})", app.syntax_theme().as_name())
        }
        (Kind::Choice(_), Value::Text(t)) if def.key == "theme.background" && t == "auto" => {
            let bg = match app.palette.background {
                super::palette::Background::Dark => "dark",
                super::palette::Background::Light => "light",
            };
            format!("auto ({bg})")
        }
        _ => v.display(),
    }
}

/// `~/…` for paths under the home directory.
fn short_path(p: &std::path::Path) -> String {
    let s = p.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && s.starts_with(&home) => format!("~{}", &s[home.len()..]),
        _ => s,
    }
}

pub fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    let rows = rows();
    // Borders, the separator and the description.
    let chrome = 3 + INFO;
    // Fit the widest label + value: cursor, gaps, a few leader dots.
    let need = SETTINGS
        .iter()
        .enumerate()
        .map(|(i, d)| width(d.label) + width(&value_text(app, i)))
        .max()
        .unwrap_or(0)
        + 14;
    let w = (need.clamp(MIN_WIDTH, MAX_WIDTH) as u16 + 2).min(area.width.saturating_sub(2));
    let h = (rows.len() as u16 + chrome)
        .min(area.height.saturating_sub(2))
        .max(area.height.min(chrome + 3));
    let popup = Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    };
    let list_h = h.saturating_sub(chrome) as usize;

    // Keep the selected row (and its section heading) in view.
    let sel = app.settings.map_or(0, |s| s.sel);
    let sel_row = rows.iter().position(|(i, _)| *i == Some(sel)).unwrap_or(0);
    let mut offset = app.settings.map_or(0, |s| s.offset);
    let heading = if sel_row > 0 && rows[sel_row - 1].0.is_none() {
        sel_row - 1
    } else {
        sel_row
    };
    offset = keep_visible(heading, offset, list_h, rows.len());
    offset = keep_visible(sel_row, offset, list_h, rows.len());
    if let Some(st) = app.settings.as_mut() {
        st.offset = offset;
    }
    let app = &*app;
    let gl = app.glyphs();

    let mut block = Block::bordered()
        .border_set(gl.border)
        .title(Span::styled(" Settings ", theme::bold()))
        .title_bottom({
            let mut spans = hint_spans(&[
                hint("←/→", "change", 1),
                hint("d", "default", 1),
                hint("e", "edit", 1),
                hint("esc", "close", 1),
            ]);
            spans.push(Span::raw(" "));
            Line::from(spans).centered()
        });
    if let Some(p) = &app.config_path {
        let path = text::truncate_start(&short_path(p), (w as usize).saturating_sub(16));
        block = block.title(Line::styled(format!(" {path} "), theme::dim()).right_aligned());
    }
    let inner = block.inner(popup);
    f.render_widget(Clear, popup);
    f.render_widget(block, popup);
    let cw = inner.width as usize;

    // Settings: cursor, label, faint leaders, value.
    let defaults = Config::default();
    let leader = if app.config.ascii { "." } else { "·" };
    let lines: Vec<Line<'static>> = rows
        .iter()
        .skip(offset)
        .take(list_h)
        .map(|(i, title)| {
            let Some(i) = *i else {
                return Line::styled(format!(" {title}"), theme::bold());
            };
            let def = &SETTINGS[i];
            let selected = i == sel;
            let changed = app.config.get(def.key) != defaults.get(def.key);
            let git = app.sources.get(def.key) == Some(&Source::Git);
            let value = value_text(app, i);
            let tag = if git { " git" } else { "" };
            let right_w = width(&value) + width(tag) + 1;
            let cursor = if selected {
                format!(" {} ", gl.cursor)
            } else {
                "   ".into()
            };
            let room = cw.saturating_sub(width(&cursor) + right_w + 3);
            let label = text::truncate_end(def.label, room);
            let dots = cw.saturating_sub(width(&cursor) + width(&label) + right_w + 2);
            let mut spans = vec![
                Span::raw(cursor),
                Span::raw(label),
                Span::raw(" "),
                Span::styled(leader.repeat(dots), theme::dim()),
                Span::raw(" "),
                Span::styled(value, if changed { theme::bold() } else { Style::new() }),
            ];
            if git {
                spans.push(Span::styled(tag, theme::banner()));
            }
            spans.push(Span::raw(" "));
            let line = Line::from(spans);
            if selected {
                line.style(theme::cursor())
            } else {
                line
            }
        })
        .collect();
    let list = Rect {
        height: list_h as u16,
        ..inner
    };
    f.render_widget(Paragraph::new(lines), list);

    // A rule joining the borders, then the selected setting's description.
    let sep_y = inner.y + list_h as u16;
    let buf = f.buffer_mut();
    buf.set_string(popup.x, sep_y, gl.tee_left, Style::new());
    buf.set_string(inner.x, sep_y, gl.rule.repeat(cw), theme::dim());
    buf.set_string(popup.x + popup.width - 1, sep_y, gl.tee_right, Style::new());
    let def = &SETTINGS[sel];
    let note = if app.sources.get(def.key) == Some(&Source::Git) {
        Line::styled(
            format!(" overridden here by git config {}", def.git),
            theme::banner(),
        )
    } else if matches!(def.kind, Kind::Text | Kind::List) {
        Line::styled(" edit this one in the file: press e", theme::dim())
    } else {
        Line::styled(format!(" {}", def.key), theme::dim())
    };
    let mut info: Vec<Line<'static>> = wrap(def.help, cw.saturating_sub(2), INFO as usize - 1)
        .into_iter()
        .map(|l| Line::raw(format!(" {l}")))
        .collect();
    info.push(note);
    let info_area = Rect {
        y: sep_y + 1,
        height: INFO,
        ..inner
    };
    f.render_widget(Paragraph::new(info), info_area);
}

/// Word-wraps `s` to `width` columns, at most `max` lines (the last one
/// truncated).
fn wrap(s: &str, width: usize, max: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        if !cur.is_empty() && text::width(&cur) + 1 + text::width(word) > width {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.len() > max {
        let rest = lines[max - 1..].join(" ");
        lines.truncate(max - 1);
        lines.push(text::truncate_end(&rest, width));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_descriptions() {
        assert_eq!(wrap("one two three four", 9, 2), ["one two", "three fo…"]);
        assert_eq!(wrap("short", 20, 2), ["short"]);
        assert_eq!(wrap("aa bb cc", 5, 3), ["aa bb", "cc"]);
    }
}

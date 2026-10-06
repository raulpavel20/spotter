//! The commit panel (`c`) and the push confirmation (`P`), drawn over the
//! screen like the settings panel.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use super::files::keep_visible;
use super::header::{Hint, hint, hint_spans};
use super::text::{self, width};
use super::theme;
use crate::app::App;
use crate::app::actions::{CommitFocus, TextInput};

/// Rows of the message body.
const BODY_ROWS: u16 = 3;
/// Lines of git output shown when something fails.
const ERROR_ROWS: usize = 6;

/// A centered, bordered panel with hints in its bottom border; returns its
/// inside.
pub(super) fn modal(
    f: &mut Frame,
    area: Rect,
    (w, h): (u16, u16),
    title: Line<'static>,
    hints: &[Hint],
    app: &App,
) -> Rect {
    let w = w.min(area.width.saturating_sub(2)).max(1);
    let h = h.min(area.height).max(3);
    let popup = Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    };
    let mut bottom = hint_spans(hints);
    bottom.push(Span::raw(" "));
    let block = Block::bordered()
        .border_set(app.glyphs().border)
        .title(title)
        .title_bottom(Line::from(bottom).centered());
    let inner = block.inner(popup);
    f.render_widget(Clear, popup);
    f.render_widget(block, popup);
    inner
}

/// A rule across the panel at row `y`, joining its borders.
/// A panel title, naming the repository when there are several.
pub(super) fn titled(title: &str, app: &App) -> Line<'static> {
    let text = match &app.label {
        Some(l) => format!("{} · {l} ", title.trim_end()),
        None => title.to_owned(),
    };
    Line::styled(text, theme::bold())
}

fn rule(f: &mut Frame, inner: Rect, y: u16, app: &App) {
    let gl = app.glyphs();
    let buf = f.buffer_mut();
    buf.set_string(inner.x - 1, y, gl.tee_left, Style::new());
    buf.set_string(
        inner.x,
        y,
        gl.rule.repeat(inner.width as usize),
        theme::dim(),
    );
    buf.set_string(inner.x + inner.width, y, gl.tee_right, Style::new());
}

/// One line of a text field, scrolled so the cursor stays in `cols`;
/// returns the text and the cursor's column on screen.
fn field_line(line: &str, cursor: Option<usize>, cols: usize) -> (String, usize) {
    let chars: Vec<char> = line.chars().collect();
    let before = |n: usize| width(&chars[..n].iter().collect::<String>());
    let mut skip = 0;
    if let Some(c) = cursor {
        while before(c) - before(skip) >= cols && skip < c {
            skip += 1;
        }
    }
    let shown: String = chars[skip..].iter().collect();
    let at = cursor.map_or(0, |c| before(c) - before(skip));
    (text::truncate_end(&shown, cols), at)
}

/// Last lines of an error, cut to the panel.
fn error_lines(e: &str, cols: usize) -> Vec<Line<'static>> {
    let lines: Vec<&str> = e.lines().collect();
    lines[lines.len().saturating_sub(ERROR_ROWS)..]
        .iter()
        .map(|l| Line::styled(format!(" {}", text::truncate_end(l, cols)), theme::error()))
        .collect()
}

pub fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    let Some(p) = &app.commit else { return };
    let gl = app.glyphs();
    let errors = p
        .error
        .as_deref()
        .map_or(0, |e| e.lines().count().min(ERROR_ROWS));
    let below = 1 + 1 + BODY_ROWS + if errors > 0 { 1 + errors as u16 } else { 0 };
    let room = area.height.saturating_sub(2 + below + 2).max(3);
    let list_h = (p.items.len() as u16).min(room);
    let w = area.width.saturating_sub(4).clamp(40, 76);
    let title = Line::from(vec![
        Span::styled(
            match &app.label {
                Some(l) => format!(" Commit · {l} "),
                None => " Commit ".into(),
            },
            theme::bold(),
        ),
        Span::styled(
            if p.busy {
                "· committing… ".to_owned()
            } else {
                format!("· {} of {} files ", p.picked(), p.items.len())
            },
            theme::dim(),
        ),
    ]);
    let hints = match (p.busy, p.focus) {
        (true, _) => vec![hint("", "committing…", 1)],
        (_, CommitFocus::Files) => vec![
            hint("space", "pick", 1),
            hint("a/n/v", "all/none/viewed", 1),
            hint("tab", "message", 1),
            hint("esc", "close", 1),
        ],
        (_, CommitFocus::Summary) => vec![
            hint("enter", "commit", 1),
            hint("tab", "body", 1),
            hint("ctrl-e", "editor", 1),
            hint("esc", "close", 1),
        ],
        (_, CommitFocus::Body) => vec![
            hint("tab", "files", 1),
            hint("ctrl-e", "editor", 1),
            hint("esc", "close", 1),
        ],
    };
    let offset = keep_visible(p.sel, p.offset, list_h as usize, p.items.len());
    if let Some(p) = app.commit.as_mut() {
        p.offset = offset;
    }
    let app = &*app;
    let Some(p) = &app.commit else { return };
    let inner = modal(f, area, (w, 2 + list_h + below), title, &hints, app);
    let cw = inner.width as usize;

    // The files: checkbox, status, path, viewed mark.
    let files_focused = p.focus == CommitFocus::Files && !p.busy;
    let rows: Vec<Line<'static>> = p
        .items
        .iter()
        .enumerate()
        .skip(offset)
        .take(list_h as usize)
        .map(|(i, item)| {
            let check = if !item.pickable() {
                "[!]"
            } else if item.picked {
                "[x]"
            } else {
                "[ ]"
            };
            let mark = if item.viewed { gl.viewed } else { " " };
            let path = text::display(&item.file.path);
            let left = format!(" {check} {} ", item.file.status.letter());
            let room = cw.saturating_sub(width(&left) + 3);
            let mut spans = vec![
                Span::raw(left),
                Span::styled(
                    text::truncate_start(&path, room),
                    theme::status(item.file.status),
                ),
            ];
            let used = text::spans_width(&spans);
            spans.push(Span::raw(" ".repeat(cw.saturating_sub(used + 2))));
            spans.push(Span::styled(mark.to_owned(), theme::viewed()));
            let line = Line::from(spans);
            if !item.pickable() {
                line.style(theme::dim())
            } else if files_focused && i == p.sel {
                line.style(theme::cursor())
            } else {
                line
            }
        })
        .collect();
    f.render_widget(
        Paragraph::new(rows),
        Rect {
            height: list_h,
            ..inner
        },
    );

    // The message: summary, then body, labels on the left.
    let label_w = 9;
    let cols = cw.saturating_sub(label_w + 1);
    let mut y = inner.y + list_h;
    rule(f, inner, y, app);
    y += 1;
    let mut cursor: Option<Position> = None;
    let label = |s: &'static str, focused: bool| {
        Span::styled(
            format!(" {s:<w$}", w = label_w - 1),
            if focused { theme::bold() } else { theme::dim() },
        )
    };
    let focused = |f: CommitFocus| p.focus == f && !p.busy;
    let x0 = inner.x + label_w as u16;
    let (summary, at) = field_line(
        &p.summary.text,
        focused(CommitFocus::Summary).then(|| p.summary.position().1),
        cols,
    );
    if focused(CommitFocus::Summary) {
        cursor = Some(Position::new(x0 + at as u16, y));
    }
    let placeholder = p.summary.text.is_empty();
    f.render_widget(
        Paragraph::new(Line::from(vec![
            label("Summary", focused(CommitFocus::Summary)),
            if placeholder {
                Span::styled("what changed, in one line", theme::dim())
            } else {
                Span::raw(summary)
            },
        ])),
        Rect {
            y,
            height: 1,
            ..inner
        },
    );
    y += 1;
    let body = body_lines(&p.body, focused(CommitFocus::Body), cols);
    for (i, (line, at)) in body.iter().enumerate() {
        let name = if i == 0 { "Body" } else { "" };
        if let Some(at) = at {
            cursor = Some(Position::new(x0 + *at as u16, y + i as u16));
        }
        f.render_widget(
            Paragraph::new(Line::from(vec![
                label(name, focused(CommitFocus::Body)),
                Span::raw(line.clone()),
            ])),
            Rect {
                y: y + i as u16,
                height: 1,
                ..inner
            },
        );
    }
    y += BODY_ROWS;

    if let Some(e) = &p.error {
        rule(f, inner, y, app);
        f.render_widget(
            Paragraph::new(error_lines(e, cw.saturating_sub(2))),
            Rect {
                y: y + 1,
                height: errors as u16,
                ..inner
            },
        );
    }
    if let Some(c) = cursor {
        f.set_cursor_position(c);
    }
}

/// The body's rows, scrolled to the cursor's line, with the cursor's
/// column on its row.
fn body_lines(t: &TextInput, focused: bool, cols: usize) -> Vec<(String, Option<usize>)> {
    let (line, col) = t.position();
    let lines: Vec<&str> = t.text.split('\n').collect();
    let rows = BODY_ROWS as usize;
    let top = if focused {
        line.saturating_sub(rows - 1)
    } else {
        0
    };
    (top..top + rows)
        .map(|i| {
            let text = lines.get(i).copied().unwrap_or_default();
            let here = focused && i == line;
            let (shown, at) = field_line(text, here.then_some(col), cols);
            (shown, here.then_some(at))
        })
        .collect()
}

pub fn draw_push(f: &mut Frame, area: Rect, app: &App) {
    let Some(p) = &app.push else { return };
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    let mut lines: Vec<Line<'static>> = match &p.upstream {
        Some(up) => vec![
            Line::from(vec![
                Span::raw(format!(" Push {} commit{} from ", p.ahead, plural(p.ahead))),
                Span::styled(p.branch.clone(), theme::bold()),
            ]),
            Line::from(vec![
                Span::raw(" to "),
                Span::styled(up.clone(), theme::bold()),
                Span::raw("?"),
            ]),
        ],
        None => vec![
            Line::from(vec![
                Span::raw(" Publish "),
                Span::styled(p.branch.clone(), theme::bold()),
                Span::raw(" to "),
                Span::styled(p.remote.clone(), theme::bold()),
                Span::raw("?"),
            ]),
            Line::styled(
                format!(
                    " A new branch there, with {} commit{}; it becomes the upstream.",
                    p.ahead,
                    plural(p.ahead)
                ),
                theme::dim(),
            ),
        ],
    };
    let w = area.width.saturating_sub(4).clamp(40, 70);
    if let Some(e) = &p.error {
        lines.push(Line::default());
        lines.extend(error_lines(e, w.saturating_sub(4) as usize));
    }
    let hints = match (p.busy, &p.error) {
        (true, _) => vec![hint("", "pushing…", 1)],
        (_, Some(_)) => vec![hint("enter", "close", 1)],
        (_, None) => vec![
            hint(
                "enter",
                if p.upstream.is_some() {
                    "push"
                } else {
                    "publish"
                },
                1,
            ),
            hint("esc", "cancel", 1),
        ],
    };
    let title = titled(" Push ", app);
    let inner = modal(f, area, (w, lines.len() as u16 + 2), title, &hints, app);
    f.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_scroll_to_the_cursor() {
        assert_eq!(field_line("hello", Some(5), 10), ("hello".into(), 5));
        // The cursor at the end of a long line stays on screen.
        let (shown, at) = field_line("abcdefghijkl", Some(12), 5);
        assert_eq!((shown.as_str(), at), ("ijkl", 4));
        assert_eq!(field_line("abcdefghijkl", None, 5).0, "abcd…");
    }
}

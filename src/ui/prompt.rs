//! A password prompt from git or ssh, over everything else.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::commit::modal;
use super::header::hint;
use super::text::{self, width};
use super::theme;
use crate::app::App;
use crate::askpass::PromptKind;

/// Lines of the prompt's own text.
const PROMPT_ROWS: usize = 3;

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let Some(p) = app.prompts.front() else { return };
    let w = area.width.saturating_sub(4).clamp(40, 70);
    let cols = w.saturating_sub(4) as usize;
    let mut lines: Vec<Line<'static>> = wrap(&p.prompt, cols)
        .into_iter()
        .map(|l| Line::raw(format!(" {l}")))
        .collect();
    let (title, hints) = match p.kind {
        PromptKind::Confirm => (
            " Confirm ",
            vec![hint("enter", "yes", 1), hint("esc", "no", 1)],
        ),
        PromptKind::Visible => (
            " git asks ",
            vec![hint("enter", "answer", 1), hint("esc", "cancel", 1)],
        ),
        PromptKind::Secret => (
            " Password ",
            vec![hint("enter", "answer", 1), hint("esc", "cancel", 1)],
        ),
    };
    let input_row = lines.len() as u16;
    if p.kind != PromptKind::Confirm {
        // What was typed; a secret only as dots, up to the cursor.
        let shown = match p.kind {
            PromptKind::Secret => "•".repeat(p.input.text.chars().count()),
            _ => p.input.text.clone(),
        };
        let shown = text::truncate_start(&shown, cols.saturating_sub(1));
        lines.push(Line::from(vec![
            Span::styled(" › ", theme::dim()),
            Span::raw(shown),
        ]));
    }
    let title = super::commit::titled(title, app);
    let inner = modal(f, area, (w, lines.len() as u16 + 2), title, &hints, app);
    let content = width(&lines.last().map(|l| l.to_string()).unwrap_or_default());
    f.render_widget(Paragraph::new(lines), inner);
    if p.kind != PromptKind::Confirm {
        let x = inner.x + (content as u16).min(inner.width.saturating_sub(1));
        f.set_cursor_position(Position::new(x, inner.y + input_row));
    }
}

/// Word-wraps the prompt to `cols`, at most a few lines.
fn wrap(s: &str, cols: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        if !cur.is_empty() && width(&cur) + 1 + width(word) > cols {
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
    if lines.len() > PROMPT_ROWS {
        let rest = lines[PROMPT_ROWS - 1..].join(" ");
        lines.truncate(PROMPT_ROWS - 1);
        lines.push(text::truncate_end(&rest, cols));
    }
    lines
        .into_iter()
        .map(|l| text::truncate_end(&l, cols))
        .collect()
}

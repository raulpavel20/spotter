//! Commit and push (`git.actions`), and the password prompts they can
//! raise: the panels' state and keys. git itself runs in main.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, ctrl};
use crate::askpass::PromptKind;
use crate::git::write::{CommitFile, PushInfo};
use crate::model::{FileChange, Status};
use crate::msg::{Effect, RefreshKind, Secret};

const OFF: &str = "commit and push are off · turn them on in settings (,)";

/// A text field with a cursor; one line, or several.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextInput {
    pub text: String,
    /// A byte offset on a char boundary.
    pub cursor: usize,
}

impl TextInput {
    pub fn new(text: &str) -> Self {
        TextInput {
            text: text.to_owned(),
            cursor: text.len(),
        }
    }

    pub fn insert(&mut self, s: &str) {
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    fn prev(&self) -> usize {
        self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next(&self) -> usize {
        self.text[self.cursor..]
            .chars()
            .next()
            .map_or(self.cursor, |c| self.cursor + c.len_utf8())
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |i| self.cursor + i)
    }

    /// Applies an editing key; `Enter` adds a line only in multi-line
    /// fields. Returns whether the key was used.
    pub fn key(&mut self, k: &KeyEvent, multiline: bool) -> bool {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        match k.code {
            KeyCode::Char('u') if ctrl => {
                let start = self.line_start();
                self.text.drain(start..self.cursor);
                self.cursor = start;
            }
            KeyCode::Char('w') if ctrl => {
                // The word before the cursor, with the spaces after it.
                let start = self.line_start();
                let before = self.text[start..self.cursor].trim_end_matches(' ');
                let word = before.rfind(' ').map_or(0, |i| i + 1);
                self.text.drain(start + word..self.cursor);
                self.cursor = start + word;
            }
            KeyCode::Char(c) if !ctrl && !alt => self.insert(c.encode_utf8(&mut [0; 4])),
            KeyCode::Backspace => {
                let p = self.prev();
                self.text.drain(p..self.cursor);
                self.cursor = p;
            }
            KeyCode::Delete => {
                let n = self.next();
                self.text.drain(self.cursor..n);
            }
            KeyCode::Left => self.cursor = self.prev(),
            KeyCode::Right => self.cursor = self.next(),
            KeyCode::Home => self.cursor = self.line_start(),
            KeyCode::End => self.cursor = self.line_end(),
            KeyCode::Up | KeyCode::Down if multiline => self.vertical(k.code == KeyCode::Up),
            KeyCode::Enter if multiline => self.insert("\n"),
            _ => return false,
        }
        true
    }

    fn vertical(&mut self, up: bool) {
        let (line, col) = self.position();
        let lines: Vec<&str> = self.text.split('\n').collect();
        let target = if up {
            line.checked_sub(1)
        } else {
            (line + 1 < lines.len()).then_some(line + 1)
        };
        let Some(t) = target else { return };
        let start: usize = lines[..t].iter().map(|l| l.len() + 1).sum();
        let off = lines[t]
            .char_indices()
            .nth(col)
            .map_or(lines[t].len(), |(i, _)| i);
        self.cursor = start + off;
    }

    /// The cursor's line, and its column in characters.
    pub fn position(&self) -> (usize, usize) {
        let start = self.line_start();
        let line = self.text[..self.cursor].matches('\n').count();
        (line, self.text[start..self.cursor].chars().count())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitItem {
    pub file: FileChange,
    pub picked: bool,
    pub viewed: bool,
}

impl CommitItem {
    /// Files in conflict can't be committed from here.
    pub fn pickable(&self) -> bool {
        self.file.status != Status::Unmerged
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitFocus {
    Files,
    Summary,
    Body,
}

/// `c`: pick files, write a message, commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitPanel {
    pub items: Vec<CommitItem>,
    pub sel: usize,
    pub offset: usize,
    pub focus: CommitFocus,
    pub summary: TextInput,
    pub body: TextInput,
    /// git is running.
    pub busy: bool,
    pub error: Option<String>,
}

impl CommitPanel {
    pub fn picked(&self) -> usize {
        self.items.iter().filter(|i| i.picked).count()
    }

    /// The summary, then the body after a blank line.
    pub fn message(&self) -> String {
        let summary = self.summary.text.trim();
        let body = self.body.text.trim_end();
        if body.trim().is_empty() {
            format!("{summary}\n")
        } else {
            format!("{summary}\n\n{}\n", body.trim_start_matches('\n'))
        }
    }
}

/// `P`: confirm a push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushPanel {
    pub remote: String,
    pub branch: String,
    /// The remote branch, `refs/heads/…`.
    pub dest: String,
    /// `remote/branch` when the branch tracks one; `None` publishes it.
    pub upstream: Option<String>,
    pub ahead: usize,
    pub busy: bool,
    pub error: Option<String>,
}

/// git or ssh asking for a password, a username or a yes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptPanel {
    pub id: u64,
    pub prompt: String,
    pub kind: PromptKind,
    pub input: TextInput,
}

/// A message from the editor: its first line, and the rest.
fn split_message(text: &str) -> (&str, &str) {
    let text = text.trim_start_matches(['\n', '\r', ' ']);
    match text.split_once('\n') {
        Some((summary, body)) => (summary.trim(), body.trim()),
        None => (text.trim(), ""),
    }
}

impl App {
    /// Whether commit and push are on; if not, says how to turn them on.
    fn actions_on(&mut self) -> bool {
        if !self.config.git_actions {
            self.toast(OFF);
        }
        self.config.git_actions
    }

    /// `c`: the commit panel, with viewed files picked.
    pub(super) fn open_commit(&mut self) -> Vec<Effect> {
        if !self.actions_on() {
            return Vec::new();
        }
        let Some(snap) = &self.snap else {
            return Vec::new();
        };
        if let Some(op) = snap.ops.first() {
            let msg = format!("{op}: finish it in git first");
            self.toast(msg);
            return Vec::new();
        }
        let items: Vec<CommitItem> = snap
            .uncommitted
            .iter()
            .map(|f| {
                let viewed = self.is_viewed(f);
                CommitItem {
                    file: f.clone(),
                    picked: viewed && f.status != Status::Unmerged,
                    viewed,
                }
            })
            .collect();
        if items.is_empty() {
            self.toast("nothing to commit");
            return Vec::new();
        }
        let (summary, body) = self.commit_draft.take().unwrap_or_default();
        let focus = if items.iter().any(|i| i.picked) {
            CommitFocus::Summary
        } else {
            CommitFocus::Files
        };
        self.commit = Some(CommitPanel {
            items,
            sel: 0,
            offset: 0,
            focus,
            summary,
            body,
            busy: false,
            error: None,
        });
        Vec::new()
    }

    pub(super) fn commit_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        let Some(p) = self.commit.as_mut() else {
            return Vec::new();
        };
        if p.busy {
            return Vec::new();
        }
        if ctrl(&k, 'e') {
            return vec![Effect::EditMessage(p.message())];
        }
        use CommitFocus::*;
        match k.code {
            KeyCode::Esc => {
                // Keep the message for next time.
                if let Some(p) = self.commit.take() {
                    self.commit_draft = Some((p.summary, p.body));
                }
                return Vec::new();
            }
            KeyCode::Tab => {
                p.focus = match p.focus {
                    Files => Summary,
                    Summary => Body,
                    Body => Files,
                };
                return Vec::new();
            }
            KeyCode::BackTab => {
                p.focus = match p.focus {
                    Files => Body,
                    Summary => Files,
                    Body => Summary,
                };
                return Vec::new();
            }
            _ => {}
        }
        match p.focus {
            Files => {
                let last = p.items.len().saturating_sub(1);
                match k.code {
                    KeyCode::Char('j') | KeyCode::Down => p.sel = (p.sel + 1).min(last),
                    KeyCode::Char('k') | KeyCode::Up => p.sel = p.sel.saturating_sub(1),
                    KeyCode::Char(' ') => {
                        if let Some(i) = p.items.get_mut(p.sel).filter(|i| i.pickable()) {
                            i.picked = !i.picked;
                        }
                    }
                    KeyCode::Char('a') => p.items.iter_mut().for_each(|i| i.picked = i.pickable()),
                    KeyCode::Char('n') => p.items.iter_mut().for_each(|i| i.picked = false),
                    KeyCode::Char('v') => p
                        .items
                        .iter_mut()
                        .for_each(|i| i.picked = i.viewed && i.pickable()),
                    KeyCode::Enter => p.focus = Summary,
                    _ => {}
                }
            }
            Summary => {
                if k.code == KeyCode::Enter {
                    return self.submit_commit();
                }
                p.summary.key(&k, false);
            }
            Body => {
                p.body.key(&k, true);
            }
        }
        Vec::new()
    }

    fn submit_commit(&mut self) -> Vec<Effect> {
        let Some(p) = self.commit.as_mut() else {
            return Vec::new();
        };
        let files: Vec<CommitFile> = p
            .items
            .iter()
            .filter(|i| i.picked)
            .map(|i| CommitFile {
                path: i.file.path.clone(),
                old_path: i.file.old_path.clone(),
                untracked: i.file.status == Status::Untracked,
            })
            .collect();
        p.error = if files.is_empty() {
            Some("pick at least one file: tab to the list, space picks".into())
        } else if p.summary.text.trim().is_empty() {
            Some("write a summary first".into())
        } else {
            None
        };
        if p.error.is_some() {
            return Vec::new();
        }
        p.busy = true;
        vec![Effect::Commit {
            files,
            message: p.message(),
        }]
    }

    pub(super) fn committed(&mut self, result: Result<String, String>) -> Vec<Effect> {
        match result {
            Ok(sha) => {
                let summary = self
                    .commit
                    .take()
                    .map(|p| p.summary.text.trim().to_owned())
                    .unwrap_or_default();
                self.commit_draft = None;
                self.own_commit = true;
                self.toast(format!("committed {sha} · {summary}"));
            }
            Err(e) => match self.commit.as_mut() {
                Some(p) => {
                    p.busy = false;
                    p.error = Some(e);
                }
                None => self.toast(e),
            },
        }
        vec![self.refresh(RefreshKind::Full)]
    }

    pub(super) fn message_edited(&mut self, text: &str) {
        if let Some(p) = self.commit.as_mut() {
            let (summary, body) = split_message(text);
            p.summary = TextInput::new(summary);
            p.body = TextInput::new(body);
            p.focus = CommitFocus::Summary;
        }
    }

    /// `P`: find out what a push would do, then confirm it.
    pub(super) fn open_push(&mut self) -> Vec<Effect> {
        if !self.actions_on() {
            return Vec::new();
        }
        vec![Effect::PreparePush]
    }

    pub(super) fn push_info(&mut self, info: Result<PushInfo, String>) -> Vec<Effect> {
        let info = match info {
            Ok(i) => i,
            Err(e) => {
                self.toast(e);
                return Vec::new();
            }
        };
        let Some(branch) = info.branch else {
            self.toast("not on a branch (detached HEAD): nothing to push");
            return Vec::new();
        };
        let Some(remote) = info.remote else {
            self.toast("no remote to push to: add one with git remote add");
            return Vec::new();
        };
        if let (Some(up), 0) = (&info.upstream, info.ahead) {
            let msg = format!("nothing to push: {up} is up to date");
            self.toast(msg);
            return Vec::new();
        }
        self.push = Some(PushPanel {
            remote,
            branch,
            dest: info.dest,
            upstream: info.upstream,
            ahead: info.ahead,
            busy: false,
            error: None,
        });
        Vec::new()
    }

    pub(super) fn push_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        let Some(p) = self.push.as_mut() else {
            return Vec::new();
        };
        if p.busy {
            return Vec::new();
        }
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('n') => self.push = None,
            KeyCode::Enter | KeyCode::Char('y') => {
                if p.error.is_some() {
                    self.push = None;
                    return Vec::new();
                }
                p.busy = true;
                return vec![Effect::Push {
                    remote: p.remote.clone(),
                    branch: p.branch.clone(),
                    dest: p.dest.clone(),
                    set_upstream: p.upstream.is_none(),
                }];
            }
            _ => {}
        }
        Vec::new()
    }

    pub(super) fn pushed(&mut self, result: Result<String, String>) -> Vec<Effect> {
        match result {
            Ok(msg) => {
                self.push = None;
                self.toast(msg);
            }
            Err(e) => match self.push.as_mut() {
                Some(p) => {
                    p.busy = false;
                    p.error = Some(e);
                }
                None => self.toast(e),
            },
        }
        vec![self.refresh(RefreshKind::Full)]
    }

    pub(super) fn ask(&mut self, id: u64, prompt: String, kind: PromptKind) {
        self.prompts.push_back(PromptPanel {
            id,
            prompt,
            kind,
            input: TextInput::default(),
        });
    }

    pub(super) fn prompt_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        let Some(p) = self.prompts.front_mut() else {
            return Vec::new();
        };
        match k.code {
            KeyCode::Esc | KeyCode::Enter => {
                let Some(p) = self.prompts.pop_front() else {
                    return Vec::new();
                };
                let answer = match (k.code, p.kind) {
                    (KeyCode::Esc, _) => None,
                    (_, PromptKind::Confirm) => Some(Secret("yes".into())),
                    _ => Some(Secret(p.input.text)),
                };
                vec![Effect::Answer { id: p.id, answer }]
            }
            _ => {
                if p.kind != PromptKind::Confirm {
                    p.input.key(&k, false);
                }
                Vec::new()
            }
        }
    }

    /// Pasted text goes into the field being typed in; a paste can't
    /// press Enter.
    pub(super) fn paste(&mut self, text: &str) {
        if let Some(p) = self.prompts.front_mut() {
            p.input.insert(text.lines().next().unwrap_or_default());
            return;
        }
        if let Some(p) = self.commit.as_mut().filter(|p| !p.busy) {
            match p.focus {
                CommitFocus::Summary => p.summary.insert(&text.replace(['\r', '\n'], " ")),
                CommitFocus::Body => p
                    .body
                    .insert(&text.replace("\r\n", "\n").replace('\r', "\n")),
                CommitFocus::Files => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn typed(input: &mut TextInput, s: &str) {
        for c in s.chars() {
            input.key(&key(KeyCode::Char(c)), true);
        }
    }

    #[test]
    fn text_input_edits_at_the_cursor() {
        let mut t = TextInput::default();
        typed(&mut t, "helo");
        t.key(&key(KeyCode::Left), false);
        typed(&mut t, "l");
        assert_eq!(t.text, "hello");
        t.key(&key(KeyCode::End), false);
        typed(&mut t, " wörld");
        t.key(&key(KeyCode::Backspace), false);
        assert_eq!(t.text, "hello wörl");
        t.key(
            &KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
            false,
        );
        assert_eq!(t.text, "hello ");
        // Lines, and moving between them.
        t.key(&key(KeyCode::Enter), true);
        typed(&mut t, "ab");
        assert_eq!(t.position(), (1, 2));
        t.key(&key(KeyCode::Up), true);
        assert_eq!(t.position(), (0, 2));
        t.key(
            &KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
            true,
        );
        assert_eq!(t.text, "llo \nab");
        // Enter does nothing in a one-line field.
        assert!(!t.key(&key(KeyCode::Enter), false));
    }

    #[test]
    fn messages_join_and_split() {
        let p = CommitPanel {
            items: Vec::new(),
            sel: 0,
            offset: 0,
            focus: CommitFocus::Summary,
            summary: TextInput::new(" Fix totals "),
            body: TextInput::new("\nRounds once.\n\n"),
            busy: false,
            error: None,
        };
        assert_eq!(p.message(), "Fix totals\n\nRounds once.\n");
        assert_eq!(
            split_message("\nFix totals\n\nRounds once.\n"),
            ("Fix totals", "Rounds once.")
        );
        assert_eq!(split_message("Only a summary"), ("Only a summary", ""));
    }
}

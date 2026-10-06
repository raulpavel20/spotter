//! Several repositories in tabs. Each tab is a whole Spotter (an [`App`])
//! for one repository; the workspace routes messages to them, notices what
//! changed where, and switches between them. One repository is a workspace
//! of one tab, shown without the tab bar.
//!
//! Like `App`, no I/O happens here: effects come back tagged with the tab
//! they are for.

pub mod discover;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use two_face::theme::EmbeddedThemeName;

use crate::app::{App, TOAST_TICKS};
use crate::git::base::BaseMode;
use crate::msg::{Effect, Msg, RefreshKind, TabId, WatchStatus};

pub struct Tab {
    pub id: TabId,
    pub name: String,
    pub app: App,
    /// What the repository looked like when you last looked at it.
    seen: Option<u64>,
    /// It changed since.
    pub activity: bool,
}

/// What the tab bar shows for one tab.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    pub loading: bool,
    /// Uncommitted changes.
    pub dirty: bool,
    /// Commits left to review on a branch (not trunk history).
    pub to_review: usize,
    pub error: bool,
    pub attention: bool,
    pub activity: bool,
}

impl Summary {
    /// Nothing to see here.
    pub fn quiet(&self) -> bool {
        !self.dirty && self.to_review == 0 && !self.error && !self.attention && !self.activity
    }
}

pub struct Workspace {
    pub tabs: Vec<Tab>,
    pub active: usize,
    /// The folder the repositories were found in.
    pub title: Option<String>,
    /// The first tab in the bar; drawing keeps the active one in view.
    pub bar_offset: usize,
    /// The highlighter's theme: the active tab's.
    hl_theme: EmbeddedThemeName,
}

impl Workspace {
    /// Tabs in order, as (name, app). The first one starts active.
    pub fn new(tabs: Vec<(String, App)>, title: Option<String>) -> Workspace {
        assert!(!tabs.is_empty(), "a workspace needs a repository");
        let several = tabs.len() > 1;
        let tabs: Vec<Tab> = tabs
            .into_iter()
            .enumerate()
            .map(|(i, (name, mut app))| {
                app.label = several.then(|| name.clone());
                app.background = i > 0;
                Tab {
                    id: TabId(i as u32),
                    name,
                    app,
                    seen: None,
                    activity: false,
                }
            })
            .collect();
        let hl_theme = tabs[0].app.syntax_theme();
        Workspace {
            tabs,
            active: 0,
            title,
            bar_offset: 0,
            hl_theme,
        }
    }

    pub fn show_bar(&self) -> bool {
        self.tabs.len() > 1
    }

    pub fn active_id(&self) -> TabId {
        self.tabs[self.active].id
    }

    pub fn active_app(&self) -> &App {
        &self.tabs[self.active].app
    }

    pub fn active_app_mut(&mut self) -> &mut App {
        &mut self.tabs[self.active].app
    }

    fn index_of(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    pub fn app(&self, id: TabId) -> Option<&App> {
        self.index_of(id).map(|i| &self.tabs[i].app)
    }

    pub fn app_mut(&mut self, id: TabId) -> Option<&mut App> {
        self.index_of(id).map(|i| &mut self.tabs[i].app)
    }

    /// The theme the highlighter should use.
    pub fn syntax_theme(&self) -> EmbeddedThemeName {
        self.hl_theme
    }

    /// A toast where you are looking.
    pub fn toast(&mut self, text: impl Into<String>) {
        self.active_app_mut().toast = Some((text.into(), TOAST_TICKS));
    }

    /// The first refresh of every tab, the one you see first.
    pub fn start(&mut self) -> Vec<(TabId, Effect)> {
        let mut order: Vec<usize> = (0..self.tabs.len()).collect();
        order.sort_by_key(|&i| i != self.active);
        let mut fx = Vec::new();
        for i in order {
            let id = self.tabs[i].id;
            fx.extend(self.tabs[i].app.start().into_iter().map(|e| (id, e)));
        }
        fx
    }

    pub fn update(&mut self, msg: Msg) -> Vec<(TabId, Effect)> {
        match msg {
            Msg::Tab(id, m) => match self.index_of(id) {
                Some(i) => self.deliver(i, *m),
                // A message for a tab that is gone.
                None => Vec::new(),
            },
            Msg::Resize(w, h) => {
                let h = h.saturating_sub(u16::from(self.show_bar()));
                (0..self.tabs.len())
                    .flat_map(|i| self.deliver(i, Msg::Resize(w, h)))
                    .collect()
            }
            Msg::Tick => (0..self.tabs.len())
                .flat_map(|i| self.deliver(i, Msg::Tick))
                .collect(),
            Msg::Key(k) => {
                if self.show_bar()
                    && k.kind == KeyEventKind::Press
                    && !self.active_app().captures_keys()
                    && let Some(fx) = self.tab_key(&k)
                {
                    return fx;
                }
                self.deliver(self.active, Msg::Key(k))
            }
            other => self.deliver(self.active, other),
        }
    }

    /// Hands a message to tab `i`, then notices what it did.
    fn deliver(&mut self, i: usize, msg: Msg) -> Vec<(TabId, Effect)> {
        let result = matches!(msg, Msg::Committed(_) | Msg::Pushed(_) | Msg::PushInfo(_));
        let refreshed = matches!(msg, Msg::Refreshed { .. });
        let active = i == self.active;
        let tab = &mut self.tabs[i];
        let (id, name) = (tab.id, tab.name.clone());
        let toast_before = tab.app.toast.clone();
        let attention_before = tab.app.needs_attention();
        let fx = tab.app.update(msg);

        if refreshed {
            let fp = tab.app.fingerprint();
            if active || tab.seen.is_none() {
                tab.seen = fp;
            } else if fp != tab.seen {
                tab.activity = true;
            }
        }
        let toast = (tab.app.toast != toast_before)
            .then(|| tab.app.toast.clone())
            .flatten();
        let attention = !attention_before && tab.app.needs_attention();

        let saved = fx.iter().any(|e| matches!(e, Effect::SaveSetting { .. }));
        let mut out = Vec::with_capacity(fx.len());
        for e in fx {
            match e {
                Effect::SetSyntaxTheme(t) if active => {
                    self.hl_theme = t;
                    out.push((id, e));
                }
                // The highlighter follows the tab you look at.
                Effect::SetSyntaxTheme(_) => {}
                e => out.push((id, e)),
            }
        }
        if !active {
            // What a commit or push did reaches you wherever you are.
            if let (true, Some((t, _))) = (result, toast) {
                self.toast(format!("{name}: {t}"));
            }
            if attention {
                let how = match i {
                    0..9 => format!("press {}", i + 1),
                    _ => "switch with < >".into(),
                };
                self.toast(format!("{name} needs you · {how}"));
            }
        }
        if saved {
            // The settings file changed under the other tabs.
            for t in &self.tabs {
                if t.id != id {
                    out.push((t.id, Effect::ReloadConfig));
                }
            }
        }
        out
    }

    /// `<` `>` and `1`–`9`.
    fn tab_key(&mut self, k: &KeyEvent) -> Option<Vec<(TabId, Effect)>> {
        let n = self.tabs.len();
        let plain = !k
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let prev = (self.active + n - 1) % n;
        let next = (self.active + 1) % n;
        let to = match k.code {
            KeyCode::Char('<') if plain => prev,
            KeyCode::Char('>') if plain => next,
            KeyCode::PageUp if ctrl => prev,
            KeyCode::PageDown if ctrl => next,
            KeyCode::Char(c @ '1'..='9') if plain => {
                let i = c as usize - '1' as usize;
                if i >= n {
                    self.toast(format!("there are only {n} repositories"));
                    return Some(Vec::new());
                }
                i
            }
            _ => return None,
        };
        Some(self.activate(to))
    }

    /// Shows tab `j`, with the session's toggles.
    pub fn activate(&mut self, j: usize) -> Vec<(TabId, Effect)> {
        if j == self.active || j >= self.tabs.len() {
            return Vec::new();
        }
        let session = self.active_app().session();
        self.tabs[self.active].app.background = true;
        self.active = j;
        let theme = self.tabs[j].app.syntax_theme();
        let tab = &mut self.tabs[j];
        let id = tab.id;
        tab.app.background = false;
        tab.activity = false;
        tab.seen = tab.app.fingerprint();
        let mut fx = Vec::new();
        if theme != self.hl_theme {
            self.hl_theme = theme;
            fx.push(Effect::SetSyntaxTheme(theme));
            fx.extend(tab.app.rehighlight());
        }
        fx.extend(tab.app.adopt_session(session));
        if tab.app.watch != WatchStatus::Live {
            // It polled slowly while hidden.
            fx.extend(tab.app.update(Msg::Fs(RefreshKind::Full)));
        }
        fx.into_iter().map(|e| (id, e)).collect()
    }

    /// Picks up viewed marks saved elsewhere; true if any changed.
    pub fn reload_marks(&mut self) -> bool {
        let mut any = false;
        for t in &mut self.tabs {
            if t.app.marks.reload_if_changed() {
                t.app.sync_viewed();
                any = true;
            }
        }
        any
    }

    pub fn summary(&self, i: usize) -> Summary {
        let t = &self.tabs[i];
        let app = &t.app;
        let snap = app.snap.as_ref();
        // Trunk history ("the last 20 commits") is not work to review.
        let branch = snap.is_some_and(|s| {
            matches!(
                s.base.mode,
                BaseMode::Feature | BaseMode::Explicit | BaseMode::TrunkUpstream
            )
        });
        Summary {
            loading: snap.is_none(),
            dirty: snap.is_some_and(|s| !s.uncommitted.is_empty()),
            to_review: if branch { app.to_review() } else { 0 },
            error: app.error.is_some(),
            attention: app.needs_attention(),
            activity: t.activity,
        }
    }
}

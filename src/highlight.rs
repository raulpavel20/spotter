//! Syntax highlighting of diff lines (syntect with bat's grammars and
//! themes, via two-face).
//!
//! Like delta, a file is highlighted with two parser states: the old side
//! reads context and `-` lines, the new side reads context and `+` lines.
//! State carries across the hunks of a file. Highlighting runs on its own
//! thread; results are cached by content key.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use bstr::{BString, ByteSlice};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use two_face::theme::{EmbeddedLazyThemeSet, EmbeddedThemeName};

use crate::model::{FilePatch, LineKind};
use crate::msg::{self, Msg, TabId};
use crate::ui::palette::Background;

/// Longer lines are not highlighted.
pub const MAX_LINE: usize = 2000;
const CACHE_ENTRIES: usize = 256;

/// A styled byte range of one line. `fg` is RGBA as the theme gives it
/// (alpha 0/1 have special meanings, see `Palette::theme_color`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    pub start: u32,
    pub end: u32,
    pub fg: [u8; 4],
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

/// Runs per hunk, per line. Empty when the file type is unknown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileHighlight {
    pub hunks: Vec<Vec<Vec<Run>>>,
}

impl FileHighlight {
    pub fn line(&self, hunk: usize, line: usize) -> &[Run] {
        self.hunks
            .get(hunk)
            .and_then(|h| h.get(line))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

/// The theme to use: `spotter.syntaxTheme` if it names a bundled theme,
/// else Monokai Extended on dark terminals and GitHub on light ones.
pub fn theme_name(configured: Option<&str>, background: Background) -> EmbeddedThemeName {
    if let Some(name) = configured {
        let wanted = name.trim();
        if let Some(found) = EmbeddedLazyThemeSet::theme_names()
            .iter()
            .find(|t| t.as_name().eq_ignore_ascii_case(wanted))
        {
            return *found;
        }
    }
    match background {
        Background::Dark => EmbeddedThemeName::MonokaiExtended,
        Background::Light => EmbeddedThemeName::Github,
    }
}

/// Choices for the settings screen: `default`, then every bundled theme.
pub fn theme_choices() -> Vec<&'static str> {
    std::iter::once("default")
        .chain(
            EmbeddedLazyThemeSet::theme_names()
                .iter()
                .map(|t| t.as_name()),
        )
        .collect()
}

pub struct Highlighter {
    syntaxes: SyntaxSet,
    theme: Theme,
}

impl Highlighter {
    pub fn new(theme: EmbeddedThemeName) -> Highlighter {
        Highlighter {
            syntaxes: two_face::syntax::extra_newlines(),
            theme: two_face::theme::extra().get(theme).clone(),
        }
    }

    /// The grammar for a path: full file name (Makefile, Dockerfile…),
    /// then extension, then the first line (shebangs).
    pub fn syntax_for(&self, path: &[u8], first_line: Option<&str>) -> Option<&SyntaxReference> {
        let path = path.to_str_lossy();
        let path = Path::new(path.as_ref());
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let ext = path.extension().map(|e| e.to_string_lossy().into_owned());
        name.and_then(|n| self.syntaxes.find_syntax_by_extension(&n))
            .or_else(|| ext.and_then(|e| self.syntaxes.find_syntax_by_extension(&e)))
            .or_else(|| first_line.and_then(|l| self.syntaxes.find_syntax_by_first_line(l)))
            .filter(|s| s.name != "Plain Text")
    }

    pub fn highlight(&self, path: &[u8], patch: &FilePatch) -> FileHighlight {
        // The first line of the new file, if a hunk starts there.
        let first = patch
            .hunks
            .first()
            .filter(|h| h.new_start <= 1)
            .and_then(|h| h.lines.iter().find(|l| l.kind != LineKind::Del))
            .and_then(|l| l.text.to_str().ok());
        let Some(syntax) = self.syntax_for(path, first) else {
            return FileHighlight::default();
        };
        let mut old = HighlightLines::new(syntax, &self.theme);
        let mut new = HighlightLines::new(syntax, &self.theme);
        let hunks = patch
            .hunks
            .iter()
            .map(|h| {
                h.lines
                    .iter()
                    .map(|l| match l.kind {
                        LineKind::NoNewline => Vec::new(),
                        LineKind::Add => self.feed(&mut new, &l.text),
                        LineKind::Del => self.feed(&mut old, &l.text),
                        LineKind::Context => {
                            self.feed(&mut old, &l.text);
                            self.feed(&mut new, &l.text)
                        }
                    })
                    .collect()
            })
            .collect();
        FileHighlight { hunks }
    }

    fn feed(&self, h: &mut HighlightLines<'_>, text: &[u8]) -> Vec<Run> {
        if text.len() > MAX_LINE {
            return Vec::new();
        }
        let Ok(s) = text.to_str() else {
            return Vec::new();
        };
        let line = format!("{s}\n");
        let Ok(regions) = h.highlight_line(&line, &self.syntaxes) else {
            return Vec::new();
        };
        let mut runs: Vec<Run> = Vec::with_capacity(regions.len());
        let mut pos = 0u32;
        for (style, piece) in regions {
            let piece = piece.strip_suffix('\n').unwrap_or(piece);
            let end = (pos as usize + piece.len()).min(s.len()) as u32;
            if end > pos {
                let c = style.foreground;
                let run = Run {
                    start: pos,
                    end,
                    fg: [c.r, c.g, c.b, c.a],
                    bold: style.font_style.contains(FontStyle::BOLD),
                    italic: style.font_style.contains(FontStyle::ITALIC),
                    underline: style.font_style.contains(FontStyle::UNDERLINE),
                };
                match runs.last_mut() {
                    Some(last)
                        if last.end == run.start
                            && (last.fg, last.bold, last.italic, last.underline)
                                == (run.fg, run.bold, run.italic, run.underline) =>
                    {
                        last.end = run.end
                    }
                    _ => runs.push(run),
                }
            }
            pos = end;
        }
        runs
    }
}

/// Messages to the highlighting thread.
#[derive(Debug, Clone)]
pub enum HlMsg {
    Highlight(HlRequest),
    /// Switch themes; cached colors are dropped.
    SetTheme(EmbeddedThemeName),
}

/// A file to highlight. `key` identifies its content and diff options,
/// and doubles as the cache key.
#[derive(Debug, Clone)]
pub struct HlRequest {
    /// The repository's tab, when there are several.
    pub tab: Option<TabId>,
    /// The tab's diff generation: only its newest diff is highlighted.
    pub seq: u64,
    pub index: usize,
    pub key: String,
    pub path: BString,
    pub patch: FilePatch,
}

/// Keys with both sides unknown (conflicted files) don't identify
/// content, so they are not cached.
fn cacheable(key: &str) -> bool {
    let mut parts = key.splitn(3, ':');
    let (old, new) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let zero = |s: &str| s.bytes().all(|b| b == b'0');
    !(zero(old) && zero(new))
}

struct Cache {
    map: HashMap<String, Arc<FileHighlight>>,
    order: VecDeque<String>,
}

impl Cache {
    fn get(&self, k: &str) -> Option<Arc<FileHighlight>> {
        self.map.get(k).cloned()
    }

    fn put(&mut self, k: String, v: Arc<FileHighlight>) {
        if !cacheable(&k) || self.map.contains_key(&k) {
            return;
        }
        self.order.push_back(k.clone());
        self.map.insert(k, v);
        while self.order.len() > CACHE_ENTRIES {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }
}

/// Only the newest diff of each tab matters. Tabs count their diffs
/// separately, so one tab's requests never drop another's.
fn keep_newest(queue: &mut VecDeque<HlRequest>) {
    let mut newest: HashMap<Option<TabId>, u64> = HashMap::new();
    for r in queue.iter() {
        let n = newest.entry(r.tab).or_default();
        *n = (*n).max(r.seq);
    }
    queue.retain(|r| newest.get(&r.tab) == Some(&r.seq));
}

fn run(mut theme: EmbeddedThemeName, rx: Receiver<HlMsg>, out: Sender<Msg>) {
    let mut hl: Option<Highlighter> = None;
    let mut cache = Cache {
        map: HashMap::new(),
        order: VecDeque::new(),
    };
    let mut queue: VecDeque<HlRequest> = VecDeque::new();
    loop {
        let mut incoming = Vec::new();
        if queue.is_empty() {
            match rx.recv() {
                Ok(m) => incoming.push(m),
                Err(_) => return,
            }
        }
        incoming.extend(rx.try_iter());
        for m in incoming {
            match m {
                HlMsg::Highlight(r) => queue.push_back(r),
                HlMsg::SetTheme(t) => {
                    if t != theme {
                        theme = t;
                        hl = None;
                        cache.map.clear();
                        cache.order.clear();
                    }
                }
            }
        }
        keep_newest(&mut queue);
        let Some(req) = queue.pop_front() else {
            continue;
        };
        let result = match cache.get(&req.key) {
            Some(h) => h,
            None => {
                let h = hl.get_or_insert_with(|| Highlighter::new(theme));
                let r = Arc::new(h.highlight(&req.path, &req.patch));
                cache.put(req.key.clone(), r.clone());
                r
            }
        };
        let msg = Msg::Highlighted {
            index: req.index,
            key: req.key,
            hl: result,
        };
        if out.send(msg::tag(req.tab, msg)).is_err() {
            return;
        }
    }
}

/// Starts the highlighting thread.
pub fn spawn(theme: EmbeddedThemeName, out: Sender<Msg>) -> Sender<HlMsg> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("spotter-syntax".into())
        .spawn(move || run(theme, rx, out))
        .expect("spawn syntax thread");
    tx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::patch::parse_patch;
    use std::sync::OnceLock;

    fn hl() -> &'static Highlighter {
        static HL: OnceLock<Highlighter> = OnceLock::new();
        HL.get_or_init(|| Highlighter::new(EmbeddedThemeName::MonokaiExtended))
    }

    #[test]
    fn keeps_the_newest_diff_of_each_tab() {
        let req = |tab: Option<u32>, seq: u64, key: &str| HlRequest {
            tab: tab.map(TabId),
            seq,
            index: 0,
            key: key.into(),
            path: "a.rs".into(),
            patch: FilePatch::default(),
        };
        let mut q: VecDeque<HlRequest> = [
            req(Some(0), 1, "old"),
            req(Some(1), 7, "other tab"),
            req(Some(0), 2, "new"),
            req(None, 3, "untagged"),
        ]
        .into();
        keep_newest(&mut q);
        let keys: Vec<&str> = q.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, ["other tab", "new", "untagged"]);
    }

    #[test]
    fn picks_grammars() {
        let h = hl();
        let name = |p: &str, first: Option<&str>| {
            h.syntax_for(p.as_bytes(), first).map(|s| s.name.clone())
        };
        assert_eq!(name("src/main.rs", None).as_deref(), Some("Rust"));
        assert!(name("scenes/player.gd", None).unwrap().contains("GDScript"));
        assert_eq!(name("Makefile", None).as_deref(), Some("Makefile"));
        assert_eq!(name("Cargo.toml", None).as_deref(), Some("TOML"));
        assert_eq!(
            name("bin/run", Some("#!/usr/bin/env python3")).as_deref(),
            Some("Python")
        );
        assert_eq!(name("notes.unknownext", None), None);
    }

    #[test]
    fn highlights_both_sides_and_covers_lines() {
        let patch = "diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,3 +1,3 @@
 fn main() {
-    let x = 1;
+    let y = \"two\";
 }
";
        let p = parse_patch(patch.as_bytes()).remove(0);
        let f = hl().highlight(b"a.rs", &p);
        for (i, line) in p.hunks[0].lines.iter().enumerate() {
            let runs = f.line(0, i);
            assert_eq!(runs.first().map(|r| r.start), Some(0), "line {i}");
            assert_eq!(
                runs.last().map(|r| r.end as usize),
                Some(line.text.len()),
                "line {i}"
            );
        }
        // `fn` and `let` are keywords, colored differently from plain text.
        let fn_kw = f.line(0, 0)[0];
        assert_eq!((fn_kw.start, fn_kw.end), (0, 2));
        let string_run = f
            .line(0, 2)
            .iter()
            .find(|r| r.start as usize == "    let y = ".len())
            .unwrap();
        assert_ne!(string_run.fg, fn_kw.fg);
    }

    #[test]
    fn unknown_types_get_nothing() {
        let patch = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n";
        let p = parse_patch(patch.as_bytes()).remove(0);
        assert!(hl().highlight(b"data.unknownext", &p).hunks.is_empty());
    }

    #[test]
    fn theme_names_resolve() {
        assert_eq!(
            theme_name(None, Background::Light),
            EmbeddedThemeName::Github
        );
        assert_eq!(
            theme_name(Some("ansi"), Background::Dark),
            EmbeddedThemeName::Ansi
        );
        assert_eq!(
            theme_name(Some("monokai extended"), Background::Light),
            EmbeddedThemeName::MonokaiExtended
        );
        assert_eq!(
            theme_name(Some("nope"), Background::Dark),
            EmbeddedThemeName::MonokaiExtended
        );
        assert!(cacheable("abc:000:p"));
        assert!(!cacheable("000:000:p"));
    }
}

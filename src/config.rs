//! Settings: CLI flags, `~/.config/spotter/config.toml`, and `spotter.*`
//! git config (per-repo overrides).
//!
//! One table, [`SETTINGS`], drives loading, git overrides, the settings
//! screen and the commented template file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bstr::ByteSlice;
use clap::Parser;

use crate::config_file;
use crate::git::cmd::{Git, Sub};

#[derive(Debug, Clone, Parser)]
#[command(
    name = "spotter",
    version,
    about = "A live, read-only review view of what changed on your branch"
)]
pub struct Args {
    /// Repositories, or folders holding some (default: the current
    /// directory). Several open as tabs.
    pub paths: Vec<PathBuf>,
    /// Base ref to compare against, overriding automatic resolution.
    #[arg(long, value_name = "REF")]
    pub base: Option<String>,
    /// Poll every 2 seconds instead of watching the file system.
    #[arg(long)]
    pub no_watch: bool,
    /// Settings file (default: ~/.config/spotter/config.toml).
    #[arg(long, value_name = "FILE")]
    pub config: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bool,
    Int {
        min: i64,
        max: i64,
        step: i64,
    },
    Choice(&'static [&'static str]),
    /// One of the bundled syntax themes, or `default`.
    Theme,
    /// Edited in the file, shown read-only in the screen.
    Text,
    /// A list of strings (patterns); file only.
    List,
}

/// One setting. `key` is `section.name` in the TOML file; `git` is the
/// lowercased `spotter.*` git config key that overrides it.
#[derive(Debug, Clone, Copy)]
pub struct Setting {
    pub key: &'static str,
    pub git: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: Kind,
}

impl Setting {
    pub fn section(&self) -> &'static str {
        self.key.split('.').next().unwrap_or("")
    }

    pub fn name(&self) -> &'static str {
        self.key.split('.').nth(1).unwrap_or("")
    }
}

const fn s(
    key: &'static str,
    git: &'static str,
    label: &'static str,
    help: &'static str,
    kind: Kind,
) -> Setting {
    Setting {
        key,
        git,
        label,
        help,
        kind,
    }
}

const BOOL: Kind = Kind::Bool;

pub const SETTINGS: &[Setting] = &[
    s(
        "review.explorer_on_open",
        "spotter.exploreronopen",
        "Open diffs with the file explorer",
        "Show the file explorer when a diff opens (wide panes only).",
        BOOL,
    ),
    s(
        "review.collapse_viewed",
        "spotter.collapseviewed",
        "Collapse files marked viewed",
        "Viewed files shrink to their header line; Enter expands them.",
        BOOL,
    ),
    s(
        "review.space_continues",
        "spotter.spacecontinues",
        "Space continues into the next commit",
        "When every file of a commit is viewed, Space opens the next commit to review.",
        BOOL,
    ),
    s(
        "review.total_includes_uncommitted",
        "spotter.totalincludesuncommitted",
        "Σ includes uncommitted changes",
        "Whether Σ Branch total starts with uncommitted work included (i toggles it).",
        BOOL,
    ),
    s(
        "diff.context_lines",
        "spotter.contextlines",
        "Context lines",
        "Unchanged lines shown around each change (git diff -U).",
        Kind::Int {
            min: 0,
            max: 20,
            step: 1,
        },
    ),
    s(
        "diff.ignore_whitespace",
        "spotter.ignorewhitespace",
        "Ignore whitespace changes",
        "Hide changes that only touch whitespace (git diff -w). W toggles it for the session.",
        BOOL,
    ),
    s(
        "diff.word_highlights",
        "spotter.wordhighlights",
        "Highlight changed words",
        "A stronger tint on the words that changed inside a modified line.",
        BOOL,
    ),
    s(
        "diff.line_numbers",
        "spotter.linenumbers",
        "Line numbers",
        "Old and new line numbers to the left of each line.",
        BOOL,
    ),
    s(
        "diff.wrap_lines",
        "spotter.wraplines",
        "Wrap long lines",
        "Start diffs with long lines wrapped instead of cut off. z toggles it while reading.",
        BOOL,
    ),
    s(
        "diff.syntax",
        "spotter.syntax",
        "Syntax highlighting",
        "Color code by language.",
        BOOL,
    ),
    s(
        "diff.tab_width",
        "spotter.tabwidth",
        "Tab width",
        "Columns per tab character.",
        Kind::Int {
            min: 1,
            max: 16,
            step: 1,
        },
    ),
    s(
        "diff.collapse_lines",
        "spotter.collapselines",
        "Collapse files above (lines)",
        "Files with more changed lines than this start collapsed.",
        Kind::Int {
            min: 100,
            max: 100_000,
            step: 100,
        },
    ),
    s(
        "diff.collapse",
        "spotter.collapse",
        "Always-collapsed patterns",
        "Gitignore-style patterns for files that start collapsed, besides lockfiles.",
        Kind::List,
    ),
    s(
        "theme.background",
        "spotter.theme",
        "Terminal background",
        "auto reads COLORFGBG and assumes dark when it is unset.",
        Kind::Choice(&["auto", "dark", "light"]),
    ),
    s(
        "theme.syntax_theme",
        "spotter.syntaxtheme",
        "Syntax theme",
        "default: Monokai Extended on dark, GitHub on light. ansi follows your terminal's colors.",
        Kind::Theme,
    ),
    s(
        "theme.ascii",
        "spotter.ascii",
        "ASCII glyphs",
        "Plain ASCII instead of symbols like ◌ ✓ Σ ▾ and box-drawing lines.",
        BOOL,
    ),
    s(
        "layout.wide_breakpoint",
        "spotter.widebreakpoint",
        "Wide layout from (columns)",
        "At this width and above, panels sit side by side.",
        Kind::Int {
            min: 60,
            max: 200,
            step: 5,
        },
    ),
    s(
        "layout.timeline_width",
        "spotter.timelinewidth",
        "Timeline width (%)",
        "Share of a wide screen given to the timeline.",
        Kind::Int {
            min: 20,
            max: 70,
            step: 5,
        },
    ),
    s(
        "layout.explorer_width",
        "spotter.explorerwidth",
        "Explorer width (%)",
        "Share of a wide screen given to the diff view's file explorer.",
        Kind::Int {
            min: 15,
            max: 60,
            step: 5,
        },
    ),
    s(
        "editor.command",
        "spotter.editor",
        "Editor command",
        "Template like `code -g {file}:{line}`; empty uses $VISUAL, then $EDITOR, then vi.",
        Kind::Text,
    ),
    s(
        "editor.gui",
        "spotter.editorgui",
        "Editor is a GUI app",
        "GUI editors start in the background; terminal ones take over the screen.",
        Kind::Choice(&["auto", "true", "false"]),
    ),
    s(
        "git.actions",
        "spotter.gitactions",
        "Commit and push from Spotter",
        "c commits the files you pick and P pushes the current branch. Off: Spotter never writes to the repository.",
        BOOL,
    ),
    s(
        "repo.trunk_depth",
        "spotter.trunkdepth",
        "Commits shown without a base",
        "On a branch with no upstream or base, show this many recent commits.",
        Kind::Int {
            min: 1,
            max: 1000,
            step: 5,
        },
    ),
];

pub fn setting(key: &str) -> Option<&'static Setting> {
    SETTINGS.iter().find(|s| s.key == key)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Text(String),
    List(Vec<String>),
}

impl Value {
    /// Parses a git config string for a setting of `kind`.
    pub fn parse(kind: Kind, s: &str) -> Result<Value, String> {
        match kind {
            Kind::Bool => parse_bool(s)
                .map(Value::Bool)
                .ok_or_else(|| format!("not a boolean: {s:?}")),
            Kind::Int { .. } => s
                .trim()
                .parse()
                .map(Value::Int)
                .map_err(|_| format!("not a number: {s:?}")),
            Kind::List => Ok(Value::List(vec![s.to_owned()])),
            _ => Ok(Value::Text(s.trim().to_owned())),
        }
    }

    /// For display in the settings screen.
    pub fn display(&self) -> String {
        match self {
            Value::Bool(true) => "on".into(),
            Value::Bool(false) => "off".into(),
            Value::Int(n) => n.to_string(),
            Value::Text(t) if t.is_empty() => "—".into(),
            Value::Text(t) => t.clone(),
            Value::List(l) if l.is_empty() => "—".into(),
            Value::List(l) => l.join(", "),
        }
    }
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        // A bare `[spotter] editorGui` with no value means true.
        "" | "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

/// Effective settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub explorer_on_open: bool,
    pub collapse_viewed: bool,
    pub space_continues: bool,
    pub total_includes_uncommitted: bool,
    pub context_lines: u32,
    pub ignore_whitespace: bool,
    pub word_highlights: bool,
    pub line_numbers: bool,
    pub wrap_lines: bool,
    pub syntax: bool,
    pub tab_width: usize,
    pub collapse_lines: u64,
    pub collapse: Vec<String>,
    /// `auto`, `dark` or `light`.
    pub background: String,
    /// A bundled theme name; `None` picks by background.
    pub syntax_theme: Option<String>,
    pub ascii: bool,
    pub wide_breakpoint: u16,
    pub timeline_width: u16,
    pub explorer_width: u16,
    pub editor: Option<String>,
    pub editor_gui: Option<bool>,
    pub git_actions: bool,
    pub trunk_depth: usize,
    /// `spotter.base` (git config only: it is per repository).
    pub base: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            explorer_on_open: true,
            collapse_viewed: true,
            space_continues: true,
            total_includes_uncommitted: true,
            context_lines: 3,
            ignore_whitespace: false,
            word_highlights: true,
            line_numbers: true,
            wrap_lines: false,
            syntax: true,
            tab_width: 4,
            collapse_lines: 1500,
            collapse: Vec::new(),
            background: "auto".into(),
            syntax_theme: None,
            ascii: false,
            wide_breakpoint: 100,
            timeline_width: 45,
            explorer_width: 30,
            editor: None,
            editor_gui: None,
            git_actions: false,
            trunk_depth: 20,
            base: None,
        }
    }
}

impl Config {
    pub fn get(&self, key: &str) -> Value {
        let text = |o: &Option<String>| Value::Text(o.clone().unwrap_or_default());
        match key {
            "review.explorer_on_open" => Value::Bool(self.explorer_on_open),
            "review.collapse_viewed" => Value::Bool(self.collapse_viewed),
            "review.space_continues" => Value::Bool(self.space_continues),
            "review.total_includes_uncommitted" => Value::Bool(self.total_includes_uncommitted),
            "diff.context_lines" => Value::Int(self.context_lines as i64),
            "diff.ignore_whitespace" => Value::Bool(self.ignore_whitespace),
            "diff.word_highlights" => Value::Bool(self.word_highlights),
            "diff.line_numbers" => Value::Bool(self.line_numbers),
            "diff.wrap_lines" => Value::Bool(self.wrap_lines),
            "diff.syntax" => Value::Bool(self.syntax),
            "diff.tab_width" => Value::Int(self.tab_width as i64),
            "diff.collapse_lines" => Value::Int(self.collapse_lines as i64),
            "diff.collapse" => Value::List(self.collapse.clone()),
            "theme.background" => Value::Text(self.background.clone()),
            "theme.syntax_theme" => Value::Text(
                self.syntax_theme
                    .clone()
                    .unwrap_or_else(|| "default".into()),
            ),
            "theme.ascii" => Value::Bool(self.ascii),
            "layout.wide_breakpoint" => Value::Int(self.wide_breakpoint as i64),
            "layout.timeline_width" => Value::Int(self.timeline_width as i64),
            "layout.explorer_width" => Value::Int(self.explorer_width as i64),
            "editor.command" => text(&self.editor),
            "editor.gui" => Value::Text(
                match self.editor_gui {
                    None => "auto",
                    Some(true) => "true",
                    Some(false) => "false",
                }
                .into(),
            ),
            "git.actions" => Value::Bool(self.git_actions),
            "repo.trunk_depth" => Value::Int(self.trunk_depth as i64),
            _ => Value::Text(String::new()),
        }
    }

    /// Sets a value, validating it against the setting's kind. Numbers
    /// are clamped to their range.
    pub fn set(&mut self, key: &str, v: Value) -> Result<(), String> {
        let def = setting(key).ok_or_else(|| format!("unknown setting {key}"))?;
        let v = match (def.kind, v) {
            (Kind::Bool, v @ Value::Bool(_)) => v,
            (Kind::Int { min, max, .. }, Value::Int(n)) => Value::Int(n.clamp(min, max)),
            (Kind::Choice(opts), Value::Text(t)) => {
                let t = t.to_ascii_lowercase();
                if !opts.contains(&t.as_str()) {
                    return Err(format!("must be one of {}", opts.join(", ")));
                }
                Value::Text(t)
            }
            (Kind::Theme | Kind::Text, v @ Value::Text(_)) => v,
            (Kind::List, v @ Value::List(_)) => v,
            (_, v) => return Err(format!("wrong type: {v:?}")),
        };
        let (b, n, t) = match &v {
            Value::Bool(b) => (*b, 0, String::new()),
            Value::Int(n) => (false, *n, String::new()),
            Value::Text(t) => (false, 0, t.clone()),
            Value::List(_) => (false, 0, String::new()),
        };
        let opt = |t: &str| Some(t.to_owned()).filter(|t| !t.is_empty());
        match key {
            "review.explorer_on_open" => self.explorer_on_open = b,
            "review.collapse_viewed" => self.collapse_viewed = b,
            "review.space_continues" => self.space_continues = b,
            "review.total_includes_uncommitted" => self.total_includes_uncommitted = b,
            "diff.context_lines" => self.context_lines = n as u32,
            "diff.ignore_whitespace" => self.ignore_whitespace = b,
            "diff.word_highlights" => self.word_highlights = b,
            "diff.line_numbers" => self.line_numbers = b,
            "diff.wrap_lines" => self.wrap_lines = b,
            "diff.syntax" => self.syntax = b,
            "diff.tab_width" => self.tab_width = n as usize,
            "diff.collapse_lines" => self.collapse_lines = n as u64,
            "diff.collapse" => {
                if let Value::List(l) = v {
                    self.collapse = l;
                }
            }
            "theme.background" => self.background = t,
            "theme.syntax_theme" => {
                self.syntax_theme = opt(&t).filter(|t| !t.eq_ignore_ascii_case("default"))
            }
            "theme.ascii" => self.ascii = b,
            "layout.wide_breakpoint" => self.wide_breakpoint = n as u16,
            "layout.timeline_width" => self.timeline_width = n as u16,
            "layout.explorer_width" => self.explorer_width = n as u16,
            "editor.command" => self.editor = opt(&t),
            "editor.gui" => {
                self.editor_gui = match t.as_str() {
                    "true" => Some(true),
                    "false" => Some(false),
                    _ => None,
                }
            }
            "git.actions" => self.git_actions = b,
            "repo.trunk_depth" => self.trunk_depth = n as usize,
            _ => {}
        }
        Ok(())
    }

    /// Defaults, then the file, then git config (`git config` already
    /// layers global and repo).
    pub fn load(file: Option<&Path>, git: &Git) -> Loaded {
        let mut loaded = Loaded::default();
        if let Some(path) = file {
            loaded.load_file(path);
        }
        let out = git
            .cmd(Sub::Config)
            .args(["-z", "--get-regexp", r"^spotter\."])
            .ok_codes(&[0, 1])
            .out()
            .unwrap_or_default();
        loaded.apply_git(&out);
        loaded
    }

    /// Only git config output (tests and callers without a file).
    pub fn parse(out: &[u8]) -> Config {
        let mut loaded = Loaded::default();
        loaded.apply_git(out);
        loaded.config
    }
}

/// Where a setting's effective value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    #[default]
    Default,
    File,
    Git,
}

#[derive(Debug, Clone, Default)]
pub struct Loaded {
    pub config: Config,
    pub sources: HashMap<&'static str, Source>,
    pub warnings: Vec<String>,
}

impl Loaded {
    pub fn source(&self, key: &str) -> Source {
        self.sources.get(key).copied().unwrap_or_default()
    }

    fn load_file(&mut self, path: &Path) {
        let doc = match config_file::read(path) {
            Ok(Some(doc)) => doc,
            Ok(None) => return,
            Err(e) => {
                self.warnings.push(format!("{}: {e}", path.display()));
                return;
            }
        };
        for def in SETTINGS {
            let Some(item) = config_file::lookup(&doc, def.key) else {
                continue;
            };
            let result =
                config_file::value_of(def.kind, item).and_then(|v| self.config.set(def.key, v));
            match result {
                Ok(()) => {
                    self.sources.insert(def.key, Source::File);
                }
                Err(e) => self
                    .warnings
                    .push(format!("{} in {}: {e}", def.key, path.display())),
            }
        }
    }

    /// Applies `git config -z --get-regexp ^spotter\.` output: `key\nvalue\0`
    /// per entry, keys lowercased. List settings collect every value.
    fn apply_git(&mut self, out: &[u8]) {
        let mut lists: HashMap<&'static str, Vec<String>> = HashMap::new();
        for entry in out.split_str("\0").filter(|e| !e.is_empty()) {
            let (key, value) = match entry.find_byte(b'\n') {
                Some(i) => (&entry[..i], &entry[i + 1..]),
                None => (entry, &b""[..]),
            };
            let key = key.to_str_lossy().to_ascii_lowercase();
            let value = value.to_str_lossy().into_owned();
            if key == "spotter.base" {
                self.config.base = Some(value).filter(|v| !v.trim().is_empty());
                continue;
            }
            let Some(def) = SETTINGS.iter().find(|s| s.git == key) else {
                continue;
            };
            if def.kind == Kind::List {
                if !value.is_empty() {
                    lists.entry(def.key).or_default().push(value);
                }
                continue;
            }
            match Value::parse(def.kind, &value).and_then(|v| self.config.set(def.key, v)) {
                Ok(()) => {
                    self.sources.insert(def.key, Source::Git);
                }
                Err(e) => self.warnings.push(format!("git config {}: {e}", def.git)),
            }
        }
        for (key, mut values) in lists {
            // Patterns add to the file's, so a repo can extend them.
            if let Value::List(mut base) = self.config.get(key) {
                base.append(&mut values);
                let _ = self.config.set(key, Value::List(base));
                self.sources.insert(key, Source::Git);
            }
        }
    }
}

/// `$XDG_CONFIG_HOME/spotter/config.toml`, else `~/.config/spotter/config.toml`.
pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("spotter").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_git_config_output() {
        let out = b"spotter.base\norigin/develop\0spotter.editorgui\ntrue\0\
spotter.collapse\n*.snap\0spotter.collapse\nvendor/**\0spotter.tabwidth\n8\0\
spotter.trunkdepth\n5\0spotter.editor\ncode -g {file}:{line}\0\
spotter.syntax\nfalse\0spotter.theme\nlight\0spotter.syntaxtheme\nGitHub\0\
spotter.contextlines\n99\0spotter.exploreronopen\nno\0";
        let cfg = Config::parse(out);
        assert_eq!(cfg.base.as_deref(), Some("origin/develop"));
        assert_eq!(cfg.editor_gui, Some(true));
        assert_eq!(cfg.collapse, ["*.snap", "vendor/**"]);
        assert_eq!(cfg.tab_width, 8);
        assert_eq!(cfg.trunk_depth, 5);
        assert_eq!(cfg.editor.as_deref(), Some("code -g {file}:{line}"));
        assert!(!cfg.syntax);
        assert_eq!(cfg.background, "light");
        assert_eq!(cfg.syntax_theme.as_deref(), Some("GitHub"));
        assert_eq!(cfg.context_lines, 20, "clamped");
        assert!(!cfg.explorer_on_open);
    }

    #[test]
    fn empty_output_is_default() {
        assert_eq!(Config::parse(b""), Config::default());
    }

    #[test]
    fn every_setting_round_trips() {
        let mut cfg = Config::default();
        for def in SETTINGS {
            let v = cfg.get(def.key);
            cfg.set(def.key, v.clone())
                .unwrap_or_else(|e| panic!("{}: {e}", def.key));
            assert_eq!(cfg.get(def.key), v, "{}", def.key);
            assert!(def.git.starts_with("spotter."), "{}", def.key);
            assert_eq!(def.git, def.git.to_ascii_lowercase());
        }
        assert_eq!(cfg, Config::default());
        let mut keys: Vec<_> = SETTINGS.iter().map(|s| s.git).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), SETTINGS.len(), "git keys are unique");
    }

    #[test]
    fn validates_values() {
        let mut cfg = Config::default();
        assert!(
            cfg.set("theme.background", Value::Text("purple".into()))
                .is_err()
        );
        assert!(cfg.set("diff.context_lines", Value::Bool(true)).is_err());
        cfg.set("theme.background", Value::Text("LIGHT".into()))
            .unwrap();
        assert_eq!(cfg.background, "light");
        cfg.set("theme.syntax_theme", Value::Text("default".into()))
            .unwrap();
        assert_eq!(cfg.syntax_theme, None);
    }

    #[test]
    fn file_then_git_layering() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "# mine\n[diff]\ncontext_lines = 5\ncollapse = [\"*.gen\"]\n[theme]\nbackground = \"light\"\n[review]\nspace_continues = \"yes\"\n",
        )
        .unwrap();
        let mut loaded = Loaded::default();
        loaded.load_file(&path);
        loaded.apply_git(b"spotter.theme\ndark\0spotter.collapse\n*.snap\0");
        let cfg = &loaded.config;
        assert_eq!(cfg.context_lines, 5);
        assert_eq!(loaded.source("diff.context_lines"), Source::File);
        assert_eq!(cfg.background, "dark");
        assert_eq!(loaded.source("theme.background"), Source::Git);
        assert_eq!(cfg.collapse, ["*.gen", "*.snap"]);
        assert_eq!(loaded.source("diff.tab_width"), Source::Default);
        // A string where a boolean belongs is reported, not fatal.
        assert!(cfg.space_continues);
        assert_eq!(loaded.warnings.len(), 1, "{:?}", loaded.warnings);
        assert!(loaded.warnings[0].contains("review.space_continues"));
    }
}

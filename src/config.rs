//! CLI flags and `spotter.*` settings from git config.

use std::path::PathBuf;

use bstr::ByteSlice;
use clap::Parser;

use crate::git::cmd::{Git, Sub};

#[derive(Debug, Clone, Parser)]
#[command(
    name = "spotter",
    version,
    about = "A live, read-only review view of what changed on your branch"
)]
pub struct Args {
    /// Repository to watch (defaults to the current directory).
    pub path: Option<PathBuf>,
    /// Base ref to compare against, overriding automatic resolution.
    #[arg(long, value_name = "REF")]
    pub base: Option<String>,
    /// Poll every 2 seconds instead of watching the file system.
    #[arg(long)]
    pub no_watch: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub base: Option<String>,
    pub editor: Option<String>,
    pub editor_gui: Option<bool>,
    pub collapse: Vec<String>,
    pub tab_width: usize,
    pub trunk_depth: usize,
    /// `spotter.syntax`: syntax highlighting in the diff view.
    pub syntax: bool,
    /// `spotter.theme`: `auto`, `dark` or `light`.
    pub theme: Option<String>,
    /// `spotter.syntaxTheme`: a bundled syntax theme name.
    pub syntax_theme: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            base: None,
            editor: None,
            editor_gui: None,
            collapse: Vec::new(),
            tab_width: 4,
            trunk_depth: 20,
            syntax: true,
            theme: None,
            syntax_theme: None,
        }
    }
}

impl Config {
    /// Reads `git config --get-regexp '^spotter\.'`. Errors leave defaults.
    pub fn load(git: &Git) -> Config {
        let out = git
            .cmd(Sub::Config)
            .args(["-z", "--get-regexp", r"^spotter\."])
            .ok_codes(&[0, 1])
            .out()
            .unwrap_or_default();
        Config::parse(&out)
    }

    /// Parses `-z` output: `key\nvalue\0` per entry. Keys arrive lowercased.
    pub fn parse(out: &[u8]) -> Config {
        let mut cfg = Config::default();
        for entry in out.split_str("\0").filter(|e| !e.is_empty()) {
            let (key, value) = match entry.find_byte(b'\n') {
                Some(i) => (&entry[..i], &entry[i + 1..]),
                None => (entry, &b""[..]),
            };
            let key = key.to_str_lossy().to_ascii_lowercase();
            let value = value.to_str_lossy().into_owned();
            match key.as_str() {
                "spotter.base" => cfg.base = Some(value).filter(|v| !v.is_empty()),
                "spotter.editor" => cfg.editor = Some(value).filter(|v| !v.is_empty()),
                "spotter.editorgui" => cfg.editor_gui = parse_bool(&value),
                "spotter.collapse" => {
                    if !value.is_empty() {
                        cfg.collapse.push(value)
                    }
                }
                "spotter.tabwidth" => {
                    if let Ok(n) = value.trim().parse::<usize>() {
                        cfg.tab_width = n.clamp(1, 16);
                    }
                }
                "spotter.syntax" => cfg.syntax = parse_bool(&value).unwrap_or(true),
                "spotter.theme" => cfg.theme = Some(value).filter(|v| !v.is_empty()),
                "spotter.syntaxtheme" => cfg.syntax_theme = Some(value).filter(|v| !v.is_empty()),
                "spotter.trunkdepth" => {
                    if let Ok(n) = value.trim().parse::<usize>() {
                        cfg.trunk_depth = n.clamp(1, 1000);
                    }
                }
                _ => {}
            }
        }
        cfg
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_git_config_output() {
        let out = b"spotter.base\norigin/develop\0spotter.editorgui\ntrue\0\
spotter.collapse\n*.snap\0spotter.collapse\nvendor/**\0spotter.tabwidth\n8\0\
spotter.trunkdepth\n5\0spotter.editor\ncode -g {file}:{line}\0\
spotter.syntax\nfalse\0spotter.theme\nlight\0spotter.syntaxtheme\nGitHub\0";
        let cfg = Config::parse(out);
        assert_eq!(cfg.base.as_deref(), Some("origin/develop"));
        assert_eq!(cfg.editor_gui, Some(true));
        assert_eq!(cfg.collapse, ["*.snap", "vendor/**"]);
        assert_eq!(cfg.tab_width, 8);
        assert_eq!(cfg.trunk_depth, 5);
        assert_eq!(cfg.editor.as_deref(), Some("code -g {file}:{line}"));
        assert!(!cfg.syntax);
        assert_eq!(cfg.theme.as_deref(), Some("light"));
        assert_eq!(cfg.syntax_theme.as_deref(), Some("GitHub"));
    }

    #[test]
    fn empty_output_is_default() {
        assert_eq!(Config::parse(b""), Config::default());
    }
}

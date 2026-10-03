//! Opening files in an editor.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::config::Config;
use crate::diffview::first_changed_line;
use crate::git::Repo;
use crate::git::patch::bytes_to_os;
use crate::msg::EditRequest;
use crate::refresh;

const GUI: &[&str] = &["code", "codium", "cursor", "zed", "subl"];

/// An editor command ready to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub argv: Vec<String>,
    pub gui: bool,
}

/// Arguments that put `{file}` at `{line}` for a known editor.
fn builtin_template(program: &str) -> &'static [&'static str] {
    let name = Path::new(program)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    match name {
        "vi" | "vim" | "nvim" | "nano" | "emacs" | "emacsclient" | "micro" | "kak" => {
            &["+{line}", "{file}"]
        }
        "hx" | "helix" | "zed" | "subl" => &["{file}:{line}"],
        "code" | "codium" | "cursor" | "code-insiders" => &["-g", "{file}:{line}"],
        _ => &["{file}"],
    }
}

fn is_gui(program: &str) -> bool {
    let name = Path::new(program)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    GUI.contains(&name.as_str())
}

/// Resolves the editor command: `spotter.editor`, then `$VISUAL`, then
/// `$EDITOR`, then `vi`.
pub fn resolve(
    cfg: &Config,
    visual: Option<String>,
    editor: Option<String>,
    file: &str,
    line: u32,
) -> Launch {
    let fill = |a: &str| {
        a.replace("{file}", file)
            .replace("{line}", &line.to_string())
    };
    let source = cfg
        .editor
        .clone()
        .or(visual.filter(|v| !v.trim().is_empty()))
        .or(editor.filter(|v| !v.trim().is_empty()))
        .unwrap_or_else(|| "vi".to_owned());
    let words: Vec<&str> = source.split_whitespace().collect();
    let program = words.first().copied().unwrap_or("vi");
    let argv: Vec<String> = if source.contains("{file}") {
        words.iter().map(|w| fill(w)).collect()
    } else {
        words
            .iter()
            .map(|w| w.to_string())
            .chain(builtin_template(program).iter().map(|a| fill(a)))
            .collect()
    };
    Launch {
        gui: cfg.editor_gui.unwrap_or_else(|| is_gui(program)),
        argv,
    }
}

/// Result of an editor request: a note for the status line.
pub enum Prepared {
    Run {
        launch: Launch,
        note: Option<String>,
    },
    Refused(String),
}

/// Works out the file, line and command for a request.
pub fn prepare(repo: &Repo, cfg: &Config, req: &EditRequest) -> Prepared {
    let full = repo.root.join(bytes_to_os(&req.file.path));
    if !full.exists() {
        return Prepared::Refused(format!(
            "{} no longer exists in the working tree",
            req.file.display_path()
        ));
    }
    let line = req.line.or_else(|| {
        // No context needed to find the first change.
        let opts = crate::git::diff::DiffOpts {
            context: 0,
            ignore_ws: false,
        };
        refresh::load_file(repo, &req.spec, &req.file, opts)
            .ok()
            .and_then(|p| first_changed_line(&p))
    });
    let mut note = None;
    if let Some(sha) = &req.commit {
        let current = refresh::hash_worktree(repo, std::slice::from_ref(&req.file.path))
            .ok()
            .and_then(|m| m.get(&req.file.path).cloned());
        if current.as_deref() != Some(req.file.new_oid.as_str()) {
            note = Some(format!("file changed since {sha} · line is approximate"));
        }
    }
    let file = full.to_string_lossy().into_owned();
    let launch = resolve(
        cfg,
        std::env::var("VISUAL").ok(),
        std::env::var("EDITOR").ok(),
        &file,
        line.unwrap_or(1),
    );
    Prepared::Run { launch, note }
}

/// Runs a terminal editor in the foreground. The caller suspends the TUI.
pub fn run_foreground(launch: &Launch, cwd: &Path) -> Result<(), String> {
    let (prog, args) = launch.argv.split_first().ok_or("empty editor command")?;
    let status = Command::new(prog)
        .args(args)
        .current_dir(cwd)
        .status()
        .map_err(|e| format!("could not run {prog}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{prog} exited with {status}"))
    }
}

/// Starts a GUI editor detached; the TUI keeps running.
pub fn spawn_detached(launch: &Launch, cwd: &Path) -> Result<(), String> {
    let (prog, args) = launch.argv.split_first().ok_or("empty editor command")?;
    let mut child = Command::new(prog)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not run {prog}: {e}"))?;
    // Reap it so it doesn't linger as a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(cfg: &Config, visual: Option<&str>, editor: Option<&str>) -> Launch {
        resolve(
            cfg,
            visual.map(Into::into),
            editor.map(Into::into),
            "/r/a b.rs",
            42,
        )
    }

    #[test]
    fn precedence_and_templates() {
        let cfg = Config::default();
        assert_eq!(r(&cfg, None, None).argv, ["vi", "+42", "/r/a b.rs"]);
        assert_eq!(
            r(&cfg, None, Some("nvim")).argv,
            ["nvim", "+42", "/r/a b.rs"]
        );
        assert_eq!(
            r(&cfg, Some("hx"), Some("nvim")).argv,
            ["hx", "/r/a b.rs:42"]
        );
        let code = r(&cfg, Some("/usr/bin/code --wait"), None);
        assert_eq!(code.argv, ["/usr/bin/code", "--wait", "-g", "/r/a b.rs:42"]);
        assert!(code.gui);
        assert!(!r(&cfg, Some("nvim"), None).gui);
        assert_eq!(
            r(&cfg, Some("myeditor"), None).argv,
            ["myeditor", "/r/a b.rs"]
        );
    }

    #[test]
    fn config_template_wins() {
        let cfg = Config {
            editor: Some("zed {file}:{line}".into()),
            editor_gui: Some(false),
            ..Config::default()
        };
        let l = r(&cfg, Some("nvim"), None);
        assert_eq!(l.argv, ["zed", "/r/a b.rs:42"]);
        assert!(!l.gui);
        let cfg = Config {
            editor: Some("emacsclient -n".into()),
            editor_gui: Some(true),
            ..Config::default()
        };
        let l = r(&cfg, None, None);
        assert_eq!(l.argv, ["emacsclient", "-n", "+42", "/r/a b.rs"]);
        assert!(l.gui);
    }
}

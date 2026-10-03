//! The settings file, `config.toml`. Edited with `toml_edit` so hand-written
//! comments and layout survive changes made from the settings screen.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use toml_edit::{Array, DocumentMut, Item, table, value};

use crate::config::{Config, Kind, SETTINGS, Value};

/// Parses the file; `Ok(None)` if it doesn't exist.
pub fn read(path: &Path) -> Result<Option<DocumentMut>, String> {
    match fs::read_to_string(path) {
        Ok(s) => s.parse::<DocumentMut>().map(Some).map_err(|e| {
            let msg = e.to_string();
            msg.lines().next().unwrap_or("invalid TOML").to_owned()
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// The item at `section.name`, if present.
pub fn lookup<'a>(doc: &'a DocumentMut, key: &str) -> Option<&'a Item> {
    let (section, name) = key.split_once('.')?;
    doc.get(section)?.get(name).filter(|i| !i.is_none())
}

/// A TOML item as a setting value of `kind`.
pub fn value_of(kind: Kind, item: &Item) -> Result<Value, String> {
    let v = item.as_value().ok_or("expected a value")?;
    match kind {
        Kind::Bool => v
            .as_bool()
            .map(Value::Bool)
            .ok_or_else(|| "expected true or false".into()),
        Kind::Int { .. } => v
            .as_integer()
            .map(Value::Int)
            .ok_or_else(|| "expected a number".into()),
        Kind::List => v
            .as_array()
            .map(|a| {
                Value::List(
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect(),
                )
            })
            .ok_or_else(|| "expected a list of strings".into()),
        Kind::Choice(_) | Kind::Theme | Kind::Text => v
            .as_str()
            .map(|s| Value::Text(s.to_owned()))
            .ok_or_else(|| "expected a string".into()),
    }
}

fn item_of(v: &Value) -> Item {
    match v {
        Value::Bool(b) => value(*b),
        Value::Int(n) => value(*n),
        Value::Text(t) => value(t.as_str()),
        Value::List(l) => {
            let mut a = Array::new();
            for s in l {
                a.push(s.as_str());
            }
            value(a)
        }
    }
}

fn toml_literal(v: &Value) -> String {
    match v {
        Value::Bool(b) => b.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Text(t) => format!("{t:?}"),
        Value::List(l) => format!(
            "[{}]",
            l.iter()
                .map(|s| format!("{s:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// A file with every setting commented out at its default, grouped by
/// section, each with its description.
pub fn template() -> String {
    let defaults = Config::default();
    let mut out = String::from(
        "# Spotter settings. Uncomment a line to change it, or press `,` in Spotter.\n\
         # Git config `spotter.*` keys (e.g. `git config spotter.contextLines 5`)\n\
         # override this file for one repository.\n",
    );
    let mut section = "";
    for def in SETTINGS {
        if def.section() != section {
            section = def.section();
            out.push_str(&format!("\n[{section}]\n"));
        }
        let mut example = toml_literal(&defaults.get(def.key));
        if matches!(def.kind, Kind::Choice(_)) {
            example.push_str(&format!(
                "  # {}",
                match def.kind {
                    Kind::Choice(o) => o.join(" | "),
                    _ => String::new(),
                }
            ));
        }
        out.push_str(&format!("# {}\n# {} = {example}\n", def.help, def.name()));
    }
    out
}

/// Creates the file from the template if it doesn't exist.
pub fn ensure_exists(path: &Path) -> io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    write_atomic(path, &template())
}

/// Sets one key (or removes it with `None`, restoring the default),
/// keeping everything else in the file as it was.
pub fn save(path: &Path, key: &str, v: Option<&Value>) -> io::Result<()> {
    let mut doc = match read(path) {
        Ok(Some(doc)) => doc,
        Ok(None) => template().parse::<DocumentMut>().expect("template parses"),
        Err(e) => return Err(io::Error::new(io::ErrorKind::InvalidData, e)),
    };
    let (section, name) = key
        .split_once('.')
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad key"))?;
    let table = doc
        .as_table_mut()
        .entry(section)
        .or_insert(table())
        .as_table_mut()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("[{section}] is not a table"),
            )
        })?;
    match v {
        Some(v) => {
            table.insert(name, item_of(v));
        }
        None => {
            table.remove(name);
        }
    }
    write_atomic(path, &doc.to_string())
}

fn write_atomic(path: &Path, content: &str) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".config.toml.{}.tmp", std::process::id()));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Loaded;

    #[test]
    fn template_lists_every_setting_and_parses_to_defaults() {
        let t = template();
        for def in SETTINGS {
            assert!(t.contains(&format!("# {} = ", def.name())), "{}", def.key);
        }
        let doc: DocumentMut = t.parse().unwrap();
        // Everything is commented out.
        for def in SETTINGS {
            assert!(lookup(&doc, def.key).is_none());
        }
    }

    #[test]
    fn save_keeps_comments_and_reset_removes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spotter").join("config.toml");
        save(&path, "diff.context_lines", Some(&Value::Int(7))).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("# Spotter settings"),
            "template written first"
        );
        assert!(text.contains("context_lines = 7"));
        // A hand-written comment survives further saves.
        fs::write(&path, format!("{text}\n# my note\n")).unwrap();
        save(
            &path,
            "theme.background",
            Some(&Value::Text("light".into())),
        )
        .unwrap();
        save(
            &path,
            "diff.collapse",
            Some(&Value::List(vec!["*.gen".into()])),
        )
        .unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my note"));
        let doc = read(&path).unwrap().unwrap();
        let mut loaded = Loaded::default();
        for def in SETTINGS {
            if let Some(item) = lookup(&doc, def.key) {
                loaded
                    .config
                    .set(def.key, value_of(def.kind, item).unwrap())
                    .unwrap();
            }
        }
        assert_eq!(loaded.config.context_lines, 7);
        assert_eq!(loaded.config.background, "light");
        assert_eq!(loaded.config.collapse, ["*.gen"]);
        save(&path, "diff.context_lines", None).unwrap();
        let doc = read(&path).unwrap().unwrap();
        assert!(lookup(&doc, "diff.context_lines").is_none());
        assert!(lookup(&doc, "theme.background").is_some());
    }

    #[test]
    fn bad_toml_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[diff\ncontext_lines = ").unwrap();
        assert!(read(&path).is_err());
        assert!(save(&path, "diff.context_lines", Some(&Value::Int(1))).is_err());
    }
}

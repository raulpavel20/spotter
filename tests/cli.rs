//! Starting the binary where there is nothing to show.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn spotter(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_spotter"))
        .args(args)
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CEILING_DIRECTORIES", dir.parent().unwrap())
        .output()
        .expect("run spotter")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn a_folder_without_repositories() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("empty");
    fs::create_dir_all(dir.join("src/deep")).unwrap();
    let o = spotter(&dir, &[]);
    assert_eq!(o.status.code(), Some(2));
    let err = stderr(&o);
    assert!(
        err.contains("no git repository in . (searched 3 levels down)"),
        "{err}"
    );
    assert!(err.contains("run spotter in a repository"), "{err}");
}

#[test]
fn a_missing_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let o = spotter(tmp.path(), &["nope"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        stderr(&o).contains("no such directory: nope"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn too_many_repositories() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("all");
    for i in 0..33 {
        fs::create_dir_all(dir.join(format!("r{i:02}/.git"))).unwrap();
    }
    let o = spotter(tmp.path(), &["all"]);
    assert_eq!(o.status.code(), Some(2));
    let err = stderr(&o);
    assert!(
        err.contains("more than 32 git repositories in all"),
        "{err}"
    );
    assert!(err.contains("pass the ones you want"), "{err}");
}

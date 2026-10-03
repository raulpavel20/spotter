//! Opening files in the editor (PLAN §6.6, M6).

mod common;

use common::{Harness, TestRepo};
use spotter::config::Config;
use spotter::editor::{self, Prepared};

#[test]
fn edit_request_uses_new_side_line_under_cursor() {
    let r = TestRepo::new();
    r.commit_file("a.txt", "1\n2\n3\n4\n5\n", "base");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.commit_file("a.txt", "1\n2\nthree\n4\n5\n", "change line 3");
    let mut h = Harness::in_memory(&r);
    // header, hunk, ctx 1, ctx 2, -3, +three
    h.keys("j enter enter ] j j j j e");
    let req = h.edits.pop().expect("editor request");
    assert_eq!(req.line, Some(3));
    assert_eq!(req.file.path, "a.txt");
    assert!(req.commit.is_some());

    // From the file list: the first changed line, found by the editor module.
    h.keys("q e");
    let req = h.edits.pop().expect("editor request");
    assert_eq!(req.line, None);
    let repo = r.repo();
    let cfg = Config {
        editor: Some("true {file} {line}".into()),
        ..Config::default()
    };
    match editor::prepare(&repo, &cfg, &req) {
        Prepared::Run { launch, note } => {
            assert_eq!(launch.argv.last().unwrap(), "3");
            assert_eq!(note, None, "file unchanged since the commit");
        }
        Prepared::Refused(m) => panic!("refused: {m}"),
    }

    // Once the working tree moves on, the line is approximate.
    r.write("a.txt", "0\n1\n2\nthree\n4\n5\n");
    match editor::prepare(&repo, &cfg, &req) {
        Prepared::Run { note, .. } => {
            assert!(note.unwrap().contains("line is approximate"));
        }
        Prepared::Refused(m) => panic!("refused: {m}"),
    }
}

#[test]
fn deleted_files_do_not_open() {
    let r = TestRepo::new();
    r.commit_file("gone.txt", "x\n", "base");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.remove("gone.txt");
    r.commit("delete");
    let mut h = Harness::in_memory(&r);
    h.keys("j enter e");
    assert!(h.edits.is_empty());
    assert!(h.app.toast.as_ref().unwrap().0.contains("deleted"));
}

//! Timeline commits with their file lists, in one `git log` call.

use bstr::ByteSlice;

use super::cmd::{Git, GitError, Sub};
use super::diff::{Cursor, LIST_ARGS, parse_entries};
use crate::model::Commit;

/// `%x1e` starts a record, `%x1f` separates fields. The header ends in NUL
/// (from `-z`), then git prints a newline before the raw records.
const FORMAT: &str = "--format=%x1e%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%s";

/// Which commits the timeline shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogRange {
    /// `<base>..HEAD`
    Since(String),
    /// The last N commits of HEAD.
    Last(usize),
    /// No commits (unborn repo).
    None,
}

pub fn timeline(git: &Git, range: &LogRange, limit: usize) -> Result<Vec<Commit>, GitError> {
    let cmd = git
        .cmd(Sub::Log)
        .arg("--topo-order")
        .arg(FORMAT)
        .args(LIST_ARGS);
    let cmd = match range {
        LogRange::None => return Ok(Vec::new()),
        LogRange::Since(base) => cmd
            .arg(format!("--max-count={limit}"))
            .arg(format!("{base}..HEAD")),
        LogRange::Last(n) => cmd
            .arg(format!("--max-count={}", n.min(&limit)))
            .arg("HEAD"),
    };
    Ok(parse_log(&cmd.arg("--").out()?))
}

pub fn parse_log(out: &[u8]) -> Vec<Commit> {
    let mut cur = Cursor::new(out);
    let mut commits = Vec::new();
    while !cur.at_end() {
        if cur.peek() != Some(0x1e) {
            cur.pos += 1;
            continue;
        }
        cur.pos += 1;
        let header = cur.until(0);
        if cur.peek() == Some(b'\n') {
            cur.pos += 1;
        }
        let f: Vec<&[u8]> = header.split_str("\x1f").collect();
        let field = |i: usize| {
            f.get(i)
                .map(|s| s.to_str_lossy().into_owned())
                .unwrap_or_default()
        };
        let files = parse_entries(&mut cur);
        commits.push(Commit {
            sha: field(0),
            parents: field(1).split_whitespace().map(str::to_owned).collect(),
            author: field(2),
            email: field(3),
            time: field(4).parse().unwrap_or(0),
            subject: field(5),
            files,
        });
    }
    commits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Status;

    #[test]
    fn parses_log_with_files_and_empty_commits() {
        let a = "2c9137ae7d2b44e17926b4b357acce863e833347";
        let b = "9bf5938b5f126331e5ba736053 7d3cad7c270592".replace(' ', "");
        let blob = "45b983be36b73c0788dc9cbcb76cbb80fc7bb057";
        let z = "0".repeat(40);
        let mut out = Vec::new();
        out.extend_from_slice(
            format!("\x1e{a}\x1f{b}\x1fT\x1ft@e\x1f1700000000\x1fsecond: x\0\n").as_bytes(),
        );
        out.extend_from_slice(format!(":100644 000000 {blob} {z} D\0d/f.txt\0").as_bytes());
        out.extend_from_slice(b"0\t1\td/f.txt\0");
        // a merge prints no diff
        out.extend_from_slice(
            format!("\x1e{b}\x1f{a} {a}\x1fT\x1ft@e\x1f1700000001\x1fMerge\0").as_bytes(),
        );
        out.extend_from_slice(
            format!("\x1e{a}\x1f\x1fT\x1ft@e\x1f1700000002\x1froot\0\n").as_bytes(),
        );
        out.extend_from_slice(format!(":000000 100644 {z} {blob} A\0\nweird\0").as_bytes());
        out.extend_from_slice(b"1\t0\t\nweird\0");
        let commits = parse_log(&out);
        assert_eq!(commits.len(), 3);
        assert_eq!(commits[0].subject, "second: x");
        assert_eq!(commits[0].files[0].status, Status::Deleted);
        assert!(commits[1].is_merge());
        assert!(commits[1].files.is_empty());
        assert!(commits[2].parents.is_empty());
        assert_eq!(commits[2].files[0].path, "\nweird");
        assert_eq!(commits[2].files[0].added, Some(1));
    }
}

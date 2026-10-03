//! `--raw --numstat -z` parsing and file-list queries.

use std::collections::HashMap;

use bstr::{BString, ByteSlice};

use super::cmd::{Git, GitError, Sub};
use crate::model::{FileChange, Status};

/// Arguments that produce the file list format this module parses.
pub const LIST_ARGS: &[&str] = &["--raw", "--numstat", "-z", "--no-abbrev"];

/// A byte cursor over NUL-separated output.
pub(crate) struct Cursor<'a> {
    pub buf: &'a [u8],
    pub pos: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Cursor { buf, pos: 0 }
    }

    pub fn peek(&self) -> Option<u8> {
        self.buf.get(self.pos).copied()
    }

    pub fn at_end(&self) -> bool {
        self.pos >= self.buf.len()
    }

    /// Reads up to (not including) `delim`, consuming the delimiter.
    pub fn until(&mut self, delim: u8) -> &'a [u8] {
        let rest = &self.buf[self.pos..];
        match rest.find_byte(delim) {
            Some(i) => {
                self.pos += i + 1;
                &rest[..i]
            }
            None => {
                self.pos = self.buf.len();
                rest
            }
        }
    }
}

struct Raw {
    old_mode: u32,
    new_mode: u32,
    old_oid: String,
    new_oid: String,
    status: Status,
    path: BString,
    old_path: Option<BString>,
}

struct Num {
    added: Option<u64>,
    deleted: Option<u64>,
    path: BString,
}

fn parse_count(s: &[u8]) -> Option<u64> {
    s.to_str().ok()?.parse().ok()
}

/// Parses raw and numstat records until EOF or a `\x1e` record separator
/// (used by the timeline log format).
pub(crate) fn parse_entries(cur: &mut Cursor<'_>) -> Vec<FileChange> {
    let mut raws = Vec::new();
    let mut nums = Vec::new();
    while let Some(b) = cur.peek() {
        match b {
            0x1e => break,
            b':' => {
                let meta = cur.until(0);
                let fields: Vec<&[u8]> = meta[1..].split_str(" ").collect();
                if fields.len() < 5 {
                    continue;
                }
                let mode =
                    |s: &[u8]| u32::from_str_radix(s.to_str().unwrap_or("0"), 8).unwrap_or(0);
                let status = Status::from_raw(fields[4]);
                let first = BString::from(cur.until(0));
                let (path, old_path) = match status {
                    Status::Renamed(_) | Status::Copied(_) => {
                        (BString::from(cur.until(0)), Some(first))
                    }
                    _ => (first, None),
                };
                raws.push(Raw {
                    old_mode: mode(fields[0]),
                    new_mode: mode(fields[1]),
                    old_oid: fields[2].to_str_lossy().into_owned(),
                    new_oid: fields[3].to_str_lossy().into_owned(),
                    status,
                    path,
                    old_path,
                });
            }
            b'0'..=b'9' | b'-' => {
                let added = parse_count(cur.until(b'\t'));
                let deleted = parse_count(cur.until(b'\t'));
                let path = if cur.peek() == Some(0) {
                    cur.pos += 1;
                    let _old = cur.until(0);
                    BString::from(cur.until(0))
                } else {
                    BString::from(cur.until(0))
                };
                nums.push(Num {
                    added,
                    deleted,
                    path,
                });
            }
            _ => {
                // Stray newline or unknown record: skip one byte.
                cur.pos += 1;
            }
        }
    }
    pair(raws, nums)
}

fn pair(raws: Vec<Raw>, nums: Vec<Num>) -> Vec<FileChange> {
    let in_order =
        raws.len() == nums.len() && raws.iter().zip(&nums).all(|(r, n)| r.path == n.path);
    let mut by_path: HashMap<BString, (Option<u64>, Option<u64>)> = HashMap::new();
    if !in_order {
        for n in &nums {
            by_path.insert(n.path.clone(), (n.added, n.deleted));
        }
    }
    raws.into_iter()
        .enumerate()
        .map(|(i, r)| {
            let (added, deleted) = if in_order {
                (nums[i].added, nums[i].deleted)
            } else {
                by_path.get(&r.path).copied().unwrap_or((Some(0), Some(0)))
            };
            FileChange {
                path: r.path,
                old_path: r.old_path,
                status: r.status,
                old_mode: r.old_mode,
                new_mode: r.new_mode,
                old_oid: r.old_oid,
                new_oid: r.new_oid,
                added,
                deleted,
                collapse: None,
                size: None,
            }
        })
        .collect()
}

pub fn parse_file_list(out: &[u8]) -> Vec<FileChange> {
    parse_entries(&mut Cursor::new(out))
}

/// What a target's diff compares.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DiffSpec {
    /// `git diff <from> <to>`
    Trees(String, String),
    /// `git diff <from>`: a tree-ish against the working tree.
    Worktree(String),
    /// `git show --remerge-diff <sha>`
    Remerge(String),
}

impl DiffSpec {
    /// The subcommand and its leading arguments.
    pub fn base_cmd<'g>(&self, git: &'g Git) -> super::cmd::Cmd<'g> {
        match self {
            DiffSpec::Trees(a, b) => git.cmd(Sub::Diff).args([a, b]),
            DiffSpec::Worktree(a) => git.cmd(Sub::Diff).arg(a),
            DiffSpec::Remerge(sha) => git
                .cmd(Sub::Show)
                .args(["--remerge-diff", "--format="])
                .arg(sha),
        }
    }

    pub fn is_volatile(&self) -> bool {
        matches!(self, DiffSpec::Worktree(_))
    }
}

pub fn file_list(git: &Git, spec: &DiffSpec) -> Result<Vec<FileChange>, GitError> {
    let out = spec.base_cmd(git).args(LIST_ARGS).out()?;
    Ok(parse_file_list(&out))
}

#[cfg(test)]
mod tests {
    use super::*;

    const Z40: &str = "0000000000000000000000000000000000000000";

    #[test]
    fn parses_raw_and_numstat_with_renames_and_binary() {
        let old = "45b983be36b73c0788dc9cbcb76cbb80fc7bb057";
        let new = "71ac1b5791204c80666ab1a4f9886b79e982739c";
        let mut out = Vec::new();
        out.extend_from_slice(format!(":100644 000000 {old} {Z40} D\0d/ü.txt\0").as_bytes());
        out.extend_from_slice(
            format!(":100644 100644 {new} {new} R092\0file one.txt\0renamed.txt\0").as_bytes(),
        );
        out.extend_from_slice(format!(":100644 100644 {old} {new} M\0bin.dat\0").as_bytes());
        out.extend_from_slice(format!(":000000 100644 {Z40} {new} A\0nl\nname\0").as_bytes());
        out.extend_from_slice(b"0\t1\td/\xc3\xbc.txt\0");
        out.extend_from_slice(b"3\t1\t\0file one.txt\0renamed.txt\0");
        out.extend_from_slice(b"-\t-\tbin.dat\0");
        out.extend_from_slice(b"1\t0\tnl\nname\0");
        let files = parse_file_list(&out);
        assert_eq!(files.len(), 4);
        assert_eq!(files[0].status, Status::Deleted);
        assert_eq!(files[0].path, "d/ü.txt");
        assert_eq!(files[0].deleted, Some(1));
        assert_eq!(files[1].status, Status::Renamed(92));
        assert_eq!(files[1].old_path.as_ref().unwrap(), "file one.txt");
        assert_eq!(files[1].path, "renamed.txt");
        assert_eq!((files[1].added, files[1].deleted), (Some(3), Some(1)));
        assert!(files[2].is_binary());
        assert_eq!(files[3].path, "nl\nname");
        assert_eq!(files[3].old_oid, Z40);
        assert_eq!(files[3].new_mode, 0o100644);
    }

    #[test]
    fn pairs_by_path_when_order_differs() {
        let n = "71ac1b5791204c80666ab1a4f9886b79e982739c";
        let mut out = Vec::new();
        out.extend_from_slice(format!(":100644 100644 {n} {n} M\0a\0").as_bytes());
        out.extend_from_slice(format!(":100644 100644 {n} {n} M\0b\0").as_bytes());
        out.extend_from_slice(b"5\t0\tb\0");
        out.extend_from_slice(b"1\t2\ta\0");
        let files = parse_file_list(&out);
        assert_eq!((files[0].added, files[0].deleted), (Some(1), Some(2)));
        assert_eq!((files[1].added, files[1].deleted), (Some(5), Some(0)));
    }

    #[test]
    fn empty_output_is_empty() {
        assert!(parse_file_list(b"").is_empty());
    }
}

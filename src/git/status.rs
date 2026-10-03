//! `git status --porcelain=v2 -z` parsing. Spotter only needs untracked
//! and unmerged entries from it; tracked changes come from `git diff`.

use bstr::{BString, ByteSlice};

use super::cmd::{Git, GitError, Sub};
use super::diff::Cursor;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusInfo {
    pub untracked: Vec<BString>,
    pub unmerged: Vec<BString>,
}

pub fn status(git: &Git) -> Result<StatusInfo, GitError> {
    let out = git
        .cmd(Sub::Status)
        .args([
            "--porcelain=v2",
            "-z",
            "--untracked-files=all",
            "--ignored=no",
            "--no-renames",
            "--ignore-submodules=none",
        ])
        .out()?;
    Ok(parse_status(&out))
}

pub fn parse_status(out: &[u8]) -> StatusInfo {
    let mut info = StatusInfo::default();
    let mut cur = Cursor::new(out);
    while !cur.at_end() {
        let record = cur.until(0);
        let Some(&kind) = record.first() else {
            continue;
        };
        // Number of space-separated fields before the path.
        let path_after = match kind {
            b'?' | b'!' => 1,
            b'1' => 8,
            b'2' => 9,
            b'u' => 10,
            _ => continue,
        };
        let path = record
            .splitn_str(path_after + 1, " ")
            .nth(path_after)
            .unwrap_or_default();
        match kind {
            b'?' => info.untracked.push(BString::from(path)),
            b'u' => info.unmerged.push(BString::from(path)),
            // A rename record is followed by its original path.
            b'2' => {
                cur.until(0);
            }
            _ => {}
        }
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_untracked_unmerged_and_skips_renames() {
        let h = "887 68efdf77ec78c9a995f9488179 3be6a4175 2b".replace(' ', "");
        let mut out = Vec::new();
        out.extend_from_slice(
            format!("1 .M N... 100644 100644 100644 {h} {h} bin.dat\0").as_bytes(),
        );
        out.extend_from_slice(
            format!("2 R. N... 100644 100644 100644 {h} {h} R100 new name\0old name\0").as_bytes(),
        );
        out.extend_from_slice(
            format!("u UU N... 100644 100644 100644 100644 {h} {h} {h} conflict file.txt\0")
                .as_bytes(),
        );
        out.extend_from_slice(b"? untracked dir/a b.txt\0");
        out.extend_from_slice(b"? nl\nname\0");
        let s = parse_status(&out);
        assert_eq!(s.untracked, ["untracked dir/a b.txt", "nl\nname"]);
        assert_eq!(s.unmerged, ["conflict file.txt"]);
    }
}

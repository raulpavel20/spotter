//! Spotter as git's and ssh's password helper.
//!
//! While a commit or push runs, `GIT_ASKPASS` and `SSH_ASKPASS` point at the
//! spotter binary itself. Started that way (with [`ENV`] set), it is a tiny
//! client: it hands the prompt to the running TUI over a private Unix
//! socket, waits for the answer typed into the prompt panel, and prints it
//! for git. Answers are never logged.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{self, DirBuilder};
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Read, Write};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::msg::{Msg, Outbox};

/// The socket's path; set only in the environment of git writes.
pub const ENV: &str = "SPOTTER_ASKPASS";

/// Prompts are short; anything longer is cut.
const MAX_PROMPT: u64 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    /// A password, passphrase or PIN: typed as dots.
    Secret,
    /// A username, or a question answered by typing (`yes`/`no`).
    Visible,
    /// Just yes or no (`SSH_ASKPASS_PROMPT=confirm`).
    Confirm,
}

pub fn classify(prompt: &str, confirm: bool) -> PromptKind {
    let p = prompt.to_lowercase();
    if confirm {
        PromptKind::Confirm
    } else if p.contains("username") || p.contains("yes/no") {
        PromptKind::Visible
    } else {
        PromptKind::Secret
    }
}

/// The environment that sends a write's prompts to this process.
pub fn env(exe: &Path, socket: &Path) -> Vec<(OsString, OsString)> {
    vec![
        ("GIT_ASKPASS".into(), exe.into()),
        ("SSH_ASKPASS".into(), exe.into()),
        // ssh asks the helper even when it has a terminal.
        ("SSH_ASKPASS_REQUIRE".into(), "force".into()),
        (ENV.into(), socket.into()),
    ]
}

/// Helper mode: asks the running Spotter and prints the answer. Returns
/// the exit code: 0 answered, 1 cancelled (git then gives up cleanly).
pub fn client(socket: &Path, prompt: &str, confirm: bool) -> u8 {
    let ask = || -> io::Result<Option<String>> {
        let mut s = UnixStream::connect(socket)?;
        s.write_all(if confirm { b"c" } else { b"p" })?;
        s.write_all(prompt.as_bytes())?;
        s.shutdown(std::net::Shutdown::Write)?;
        let mut reply = String::new();
        s.read_to_string(&mut reply)?;
        Ok(reply.strip_prefix("ok\n").map(str::to_owned))
    };
    match ask() {
        Ok(Some(answer)) => {
            println!("{answer}");
            0
        }
        _ => 1,
    }
}

type Pending = Arc<Mutex<HashMap<u64, Sender<Option<String>>>>>;

/// The listening side, in the TUI. Each prompt arrives as
/// [`Msg::AskPass`]; [`Server::answer`] replies to it.
pub struct Server {
    dir: PathBuf,
    socket: PathBuf,
    pending: Pending,
}

impl Server {
    /// Listens on a socket in a fresh directory only this user can open.
    pub fn start(tx: impl Into<Outbox>) -> io::Result<Server> {
        let tx = tx.into();
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|d| d.is_dir())
            .unwrap_or_else(std::env::temp_dir);
        let tag = std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish();
        let dir = base.join(format!("spotter-{}-{tag:x}", std::process::id()));
        DirBuilder::new().mode(0o700).create(&dir)?;
        let socket = dir.join("askpass.sock");
        let listener = UnixListener::bind(&socket)?;
        let pending: Pending = Arc::default();
        let next = Arc::new(AtomicU64::new(1));
        let p = pending.clone();
        thread::Builder::new()
            .name("spotter-askpass".into())
            .spawn(move || {
                for conn in listener.incoming().flatten() {
                    let (tx, p, next) = (tx.clone(), p.clone(), next.clone());
                    thread::spawn(move || {
                        let _ = serve(conn, &tx, &p, &next);
                    });
                }
            })?;
        Ok(Server {
            dir,
            socket,
            pending,
        })
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Replies to prompt `id`; `None` cancels it.
    pub fn answer(&self, id: u64, answer: Option<String>) {
        let reply = self.pending.lock().ok().and_then(|mut p| p.remove(&id));
        if let Some(reply) = reply {
            let _ = reply.send(answer);
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket);
        let _ = fs::remove_dir(&self.dir);
    }
}

/// One helper's request: a kind byte, then the prompt.
fn serve(mut conn: UnixStream, tx: &Outbox, pending: &Pending, next: &AtomicU64) -> io::Result<()> {
    let mut req = Vec::new();
    (&mut conn).take(MAX_PROMPT).read_to_end(&mut req)?;
    let (flag, prompt) = req.split_first().unwrap_or((&b'p', &[]));
    let prompt = String::from_utf8_lossy(prompt).trim().to_owned();
    let kind = classify(&prompt, *flag == b'c');
    let id = next.fetch_add(1, Ordering::SeqCst);
    let (reply_tx, reply_rx) = mpsc::channel();
    if let Ok(mut p) = pending.lock() {
        p.insert(id, reply_tx);
    }
    crate::log::line(|| format!("askpass: prompt {id} ({kind:?}): {prompt}"));
    if tx.send(Msg::AskPass { id, prompt, kind }).is_err() {
        return Ok(());
    }
    match reply_rx.recv().ok().flatten() {
        Some(answer) => conn.write_all(format!("ok\n{answer}").as_bytes()),
        None => conn.write_all(b"no\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_prompts() {
        use PromptKind::*;
        assert_eq!(
            classify("Password for 'https://x@github.com': ", false),
            Secret
        );
        assert_eq!(
            classify(
                "Enter passphrase for key '/home/a/.ssh/id_ed25519': ",
                false
            ),
            Secret
        );
        assert_eq!(
            classify("Username for 'https://github.com': ", false),
            Visible
        );
        assert_eq!(
            classify(
                "Are you sure you want to continue connecting (yes/no/[fingerprint])?",
                false
            ),
            Visible
        );
        assert_eq!(classify("Allow use of key?", true), Confirm);
    }

    #[test]
    fn round_trip_through_the_socket() {
        let (tx, rx) = mpsc::channel();
        let server = Server::start(tx).unwrap();
        let path = server.socket().to_owned();
        let client = thread::spawn(move || {
            let mut s = UnixStream::connect(&path).unwrap();
            s.write_all(b"pPassword: ").unwrap();
            s.shutdown(std::net::Shutdown::Write).unwrap();
            let mut reply = String::new();
            s.read_to_string(&mut reply).unwrap();
            reply
        });
        let Ok(Msg::AskPass { id, prompt, kind }) = rx.recv() else {
            panic!("no prompt");
        };
        assert_eq!((prompt.as_str(), kind), ("Password:", PromptKind::Secret));
        server.answer(id, Some("hunter2".into()));
        assert_eq!(client.join().unwrap(), "ok\nhunter2");
        // The socket's directory is private.
        use std::os::unix::fs::PermissionsExt;
        let dir = server.socket().parent().unwrap();
        let mode = fs::metadata(dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}

//! Everything that talks to git. All calls go through [`cmd::Git`].

pub mod base;
pub mod cmd;
pub mod diff;
pub mod log;
pub mod patch;
pub mod repo;
pub mod status;

pub use cmd::{Git, GitError, Sub};
pub use repo::{Repo, RepoOp};

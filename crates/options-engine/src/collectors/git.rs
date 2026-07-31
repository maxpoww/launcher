//! Layer 4 (part) — git context of the focused window.
//!
//! A **deriver**: it reads the current aggregate (the focused window's PID),
//! resolves that process's working directory via `/proc/{pid}/cwd`, walks up to
//! the enclosing repository, and reports its branch — all by reading files, no
//! `git` subprocess. It recomputes only when the focused PID actually changes,
//! so emitting a result can never feed itself into a loop.
//!
//! `is_dirty` is not yet computed: a correct answer means comparing the working
//! tree against the index (essentially `git status`), which is deferred to a
//! dedicated step (likely libgit2) rather than approximated here.

use std::path::{Path, PathBuf};

use tokio::sync::{mpsc, watch};

use crate::collector::{Collector, CollectorFuture};
use crate::message::{ContextDelta, Update};
use crate::state::{ContextState, GitContext, Layer};

#[derive(Default)]
pub struct GitCollector;

impl GitCollector {
    pub fn new() -> Self {
        Self
    }
}

impl Collector for GitCollector {
    fn name(&self) -> &'static str {
        "git"
    }
    fn layer(&self) -> Layer {
        Layer::Hardware
    }
    fn run(
        self: Box<Self>,
        mut ctx: watch::Receiver<ContextState>,
        tx: mpsc::Sender<Update>,
    ) -> CollectorFuture {
        Box::pin(async move {
            let mut last_pid: Option<u32> = None;
            loop {
                let pid = ctx.borrow_and_update().window.pid;
                if Some(pid) != last_pid {
                    last_pid = Some(pid);
                    let git = if pid == 0 {
                        GitContext::default() // nothing focused → no repo
                    } else {
                        git_for_pid(pid).unwrap_or_default()
                    };
                    if tx
                        .send(Update::Delta(Layer::Hardware, ContextDelta::Git(git)))
                        .await
                        .is_err()
                    {
                        return Ok(()); // aggregator gone
                    }
                }
                // Wait for the next context change; recompute only if PID moved.
                if ctx.changed().await.is_err() {
                    return Ok(());
                }
            }
        })
    }
}

/// Git context for the process `pid`'s working directory, or `None` when it
/// isn't inside a repository (or `/proc` isn't readable for it).
fn git_for_pid(pid: u32) -> Option<GitContext> {
    let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    let (repo_root, git_dir) = find_git_dir(&cwd)?;
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    Some(GitContext {
        repo_root: Some(repo_root),
        branch: parse_head(&head),
        is_dirty: false, // TODO: proper working-tree/index comparison
    })
}

/// Walk up from `start` to the enclosing repo, returning `(repo_root, git_dir)`.
/// Handles both a `.git` directory and a `.git` *file* (`gitdir: …`, used by
/// worktrees and submodules).
fn find_git_dir(start: &Path) -> Option<(PathBuf, PathBuf)> {
    for anc in start.ancestors() {
        let dotgit = anc.join(".git");
        if dotgit.is_dir() {
            return Some((anc.to_path_buf(), dotgit));
        }
        if dotgit.is_file() {
            let content = std::fs::read_to_string(&dotgit).ok()?;
            let rel = content.strip_prefix("gitdir:")?.trim();
            // `join` with an absolute path replaces; a relative one resolves
            // against the repo root.
            return Some((anc.to_path_buf(), anc.join(rel)));
        }
    }
    None
}

/// The branch name from a `HEAD` file's contents, or `None` for a detached
/// HEAD. Preserves slashes in branch names (`feature/foo`).
fn parse_head(head: &str) -> Option<String> {
    let head = head.trim();
    let reference = head.strip_prefix("ref:")?.trim();
    Some(
        reference
            .strip_prefix("refs/heads/")
            .unwrap_or(reference)
            .to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_symbolic_head_to_branch() {
        assert_eq!(parse_head("ref: refs/heads/main\n").as_deref(), Some("main"));
    }

    #[test]
    fn keeps_slashes_in_branch_names() {
        assert_eq!(
            parse_head("ref: refs/heads/feature/nice-thing\n").as_deref(),
            Some("feature/nice-thing")
        );
    }

    #[test]
    fn detached_head_has_no_branch() {
        assert_eq!(parse_head("9f8e7d6c5b4a39281706\n"), None);
    }

    #[test]
    fn finds_repo_root_from_nested_dir() {
        // Build a throwaway repo-ish tree: <base>/.git and <base>/a/b.
        let base = std::env::temp_dir().join(format!(
            "opt-engine-git-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let nested = base.join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(base.join(".git")).unwrap();

        let (root, git_dir) = find_git_dir(&nested).expect("should find repo");
        assert_eq!(root, base);
        assert_eq!(git_dir, base.join(".git"));

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn no_repo_above_returns_none() {
        // A path with no .git anywhere up to root.
        assert!(find_git_dir(Path::new("/proc")).is_none());
    }
}

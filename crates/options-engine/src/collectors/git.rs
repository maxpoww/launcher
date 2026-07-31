//! Layer 4 (part) — git context of the focused window.
//!
//! A **deriver**: it reads the current aggregate (the focused window's PID),
//! resolves that process's working directory via `/proc/{pid}/cwd`, walks up to
//! the enclosing repository, and reports its branch and dirty state.
//!
//! Branch/root come from reading files (`.git/HEAD`); dirty state comes from
//! `git status --porcelain` (libgit2 isn't in the dev shell, and reimplementing
//! status correctly — index vs worktree, gitignore, staged/untracked — is a
//! rabbit hole; the subprocess is correct and cheap since it only runs on a
//! repo change or a short throttle). Recomputes when the focused PID changes or
//! every [`REFRESH`] while in a repo, deduplicated so it never self-feeds.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::process::Command;
use tokio::sync::{mpsc, watch};

use crate::collector::{Collector, CollectorFuture};
use crate::message::{ContextDelta, Update};
use crate::state::{ContextState, GitContext, Layer};

/// While in a repo, re-check dirty state at least this often (a file edit
/// doesn't change the focused window, so focus-change alone would go stale).
const REFRESH: Duration = Duration::from_secs(3);

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
            let mut last_git = GitContext::default();
            let mut last_refresh = Instant::now();
            loop {
                let pid = ctx.borrow_and_update().window.pid;
                let pid_changed = Some(pid) != last_pid;
                // Only poll dirty state while we're actually in a repo.
                let refresh_due =
                    last_git.repo_root.is_some() && last_refresh.elapsed() >= REFRESH;
                if pid_changed || refresh_due {
                    last_pid = Some(pid);
                    last_refresh = Instant::now();
                    let git = if pid == 0 {
                        GitContext::default() // nothing focused → no repo
                    } else {
                        git_for_pid(pid).await.unwrap_or_default()
                    };
                    // Deduplicate: the throttled refresh would otherwise re-emit
                    // an unchanged context every few seconds.
                    if git != last_git {
                        last_git = git.clone();
                        if tx
                            .send(Update::Delta(Layer::Hardware, ContextDelta::Git(git)))
                            .await
                            .is_err()
                        {
                            return Ok(()); // aggregator gone
                        }
                    }
                }
                // Wait for the next context change (system poll wakes us ~3s).
                if ctx.changed().await.is_err() {
                    return Ok(());
                }
            }
        })
    }
}

/// Git context for the process `pid`'s working directory, or `None` when it
/// isn't inside a repository (or `/proc` isn't readable for it).
async fn git_for_pid(pid: u32) -> Option<GitContext> {
    let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    let (repo_root, git_dir) = find_git_dir(&cwd)?;
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let is_dirty = repo_is_dirty(&repo_root).await;
    Some(GitContext {
        branch: parse_head(&head),
        is_dirty,
        repo_root: Some(repo_root),
    })
}

/// Whether the working tree at `root` has changes — via `git status --porcelain`
/// (respects .gitignore, staged/unstaged, and untracked files). Any failure
/// (no `git`, not a work tree) reads as clean.
async fn repo_is_dirty(root: &Path) -> bool {
    let Ok(out) = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain", "--untracked-files=normal"])
        .output()
        .await
    else {
        return false;
    };
    out.status.success() && dirty_from_porcelain(&String::from_utf8_lossy(&out.stdout))
}

/// `git status --porcelain` prints one line per change; empty output = clean.
fn dirty_from_porcelain(output: &str) -> bool {
    !output.trim().is_empty()
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

    #[test]
    fn porcelain_empty_is_clean() {
        assert!(!dirty_from_porcelain("\n  \n"));
        assert!(dirty_from_porcelain(" M src/main.rs\n?? new.txt\n"));
    }

    #[tokio::test]
    async fn repo_dirty_reflects_worktree_changes() {
        let base = std::env::temp_dir().join(format!(
            "opt-engine-dirty-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&base)
                .args(args)
                .output()
                .unwrap()
        };
        git(&["init"]);
        // A fresh repo with nothing in it is clean.
        assert!(!repo_is_dirty(&base).await);
        // An untracked file makes it dirty.
        std::fs::write(base.join("hello.txt"), "hi").unwrap();
        assert!(repo_is_dirty(&base).await);

        std::fs::remove_dir_all(&base).ok();
    }
}

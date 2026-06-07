use anyhow::Result;

use super::GitCommands;
use crate::model::Worktree;

impl GitCommands {
    pub fn load_worktrees(&self) -> Result<Vec<Worktree>> {
        let result = self
            .git()
            .args(&["worktree", "list", "--porcelain"])
            .run()?;
        if !result.success {
            return Ok(Vec::new());
        }

        let mut worktrees = Vec::new();
        let mut path = String::new();
        let mut branch = String::new();
        let mut hash = String::new();
        let mut is_bare = false;

        for line in result.stdout.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                if !path.is_empty() && !is_bare {
                    worktrees.push(Worktree {
                        path: path.clone(),
                        branch: branch.clone(),
                        hash: hash.clone(),
                        is_current: false,
                        is_main: worktrees.is_empty(),
                    });
                }
                path = p.to_string();
                branch.clear();
                hash.clear();
                is_bare = false;
            } else if let Some(h) = line.strip_prefix("HEAD ") {
                hash = h.to_string();
            } else if let Some(b) = line.strip_prefix("branch ") {
                branch = b.strip_prefix("refs/heads/").unwrap_or(b).to_string();
            } else if line == "bare" {
                is_bare = true;
            } else if line == "detached" {
                branch = "(detached)".to_string();
            }
        }

        // Push the last one
        if !path.is_empty() && !is_bare {
            worktrees.push(Worktree {
                path: path.clone(),
                branch: branch.clone(),
                hash: hash.clone(),
                is_current: false,
                is_main: worktrees.is_empty(),
            });
        }

        // Mark the current worktree
        let repo_path = self.repo_path().to_string_lossy().to_string();
        for wt in &mut worktrees {
            if wt.path == repo_path {
                wt.is_current = true;
            }
        }

        Ok(worktrees)
    }

    pub fn create_worktree_new_branch(
        &self,
        path: &str,
        new_branch: &str,
        base_ref: Option<&str>,
    ) -> Result<()> {
        let mut cmd = self
            .git()
            .args(&["worktree", "add", "-b", new_branch, path]);
        if let Some(base_ref) = base_ref
            .map(str::trim)
            .filter(|base_ref| !base_ref.is_empty())
        {
            cmd = cmd.arg(base_ref);
        }
        cmd.run_expecting_success()?;
        Ok(())
    }

    pub fn create_worktree_existing_branch(&self, path: &str, branch: &str) -> Result<()> {
        self.git()
            .args(&["worktree", "add", path, branch])
            .run_expecting_success()?;
        Ok(())
    }

    pub fn remove_worktree(&self, path: &str, force: bool) -> Result<()> {
        let mut cmd = self.git();
        cmd = cmd.args(&["worktree", "remove", path]);
        if force {
            cmd = cmd.arg("--force");
        }
        cmd.run_expecting_success()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestRepo {
        root: PathBuf,
        initial_branch: String,
    }

    impl TestRepo {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "lazygitrs-worktree-test-{}-{}",
                std::process::id(),
                unique
            ));
            fs::create_dir_all(&root).unwrap();
            run_git(&root, &["init"]);
            run_git(&root, &["config", "user.name", "Test User"]);
            run_git(&root, &["config", "user.email", "test@example.com"]);
            fs::write(root.join("README.md"), "initial\n").unwrap();
            run_git(&root, &["add", "README.md"]);
            run_git(&root, &["commit", "-m", "initial"]);
            let initial_branch = current_branch(&root);
            Self {
                root,
                initial_branch,
            }
        }

        fn git(&self) -> GitCommands {
            GitCommands::new(&self.root).unwrap()
        }

        fn path(&self, name: &str) -> PathBuf {
            let repo_name = self.root.file_name().unwrap().to_string_lossy();
            self.root
                .parent()
                .unwrap()
                .join(format!("{repo_name}-{name}"))
        }
    }

    impl Drop for TestRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn run_git(cwd: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn current_branch(path: &Path) -> String {
        run_git(path, &["rev-parse", "--abbrev-ref", "HEAD"])
    }

    #[test]
    fn creates_existing_branch_worktree() {
        let repo = TestRepo::new();
        run_git(&repo.root, &["checkout", "-b", "existing-worktree-branch"]);
        fs::write(repo.root.join("existing.txt"), "from existing branch\n").unwrap();
        run_git(&repo.root, &["add", "existing.txt"]);
        run_git(&repo.root, &["commit", "-m", "existing branch commit"]);
        run_git(&repo.root, &["checkout", &repo.initial_branch]);
        let worktree_path = repo.path("lazygitrs existing branch worktree");

        repo.git()
            .create_worktree_existing_branch(
                worktree_path.to_str().unwrap(),
                "existing-worktree-branch",
            )
            .unwrap();

        assert_eq!(current_branch(&worktree_path), "existing-worktree-branch");
        assert!(worktree_path.join("existing.txt").exists());
        let _ = fs::remove_dir_all(&worktree_path);
    }

    #[test]
    fn creates_new_branch_worktree_without_explicit_base_ref() {
        let repo = TestRepo::new();
        let worktree_path = repo.path("lazygitrs worktree no base");

        repo.git()
            .create_worktree_new_branch(worktree_path.to_str().unwrap(), "worktree-no-base", None)
            .unwrap();

        assert_eq!(current_branch(&worktree_path), "worktree-no-base");
        assert!(worktree_path.join("README.md").exists());
        let _ = fs::remove_dir_all(&worktree_path);
    }

    #[test]
    fn creates_new_branch_worktree_with_explicit_base_ref() {
        let repo = TestRepo::new();
        run_git(&repo.root, &["checkout", "-b", "base-ref"]);
        fs::write(repo.root.join("base.txt"), "from base\n").unwrap();
        run_git(&repo.root, &["add", "base.txt"]);
        run_git(&repo.root, &["commit", "-m", "base commit"]);
        run_git(&repo.root, &["checkout", &repo.initial_branch]);
        let worktree_path = repo.path("lazygitrs worktree explicit base");

        repo.git()
            .create_worktree_new_branch(
                worktree_path.to_str().unwrap(),
                "worktree-from-base",
                Some("base-ref"),
            )
            .unwrap();

        assert_eq!(current_branch(&worktree_path), "worktree-from-base");
        assert!(worktree_path.join("base.txt").exists());
        let _ = fs::remove_dir_all(&worktree_path);
    }

    #[test]
    fn creates_new_branch_worktree_at_path_containing_spaces() {
        let repo = TestRepo::new();
        let worktree_path = repo.path("lazygitrs worktree path with spaces");

        repo.git()
            .create_worktree_new_branch(
                worktree_path.to_str().unwrap(),
                "worktree-spaces",
                Some("   "),
            )
            .unwrap();

        assert_eq!(current_branch(&worktree_path), "worktree-spaces");
        assert!(worktree_path.exists());
        let _ = fs::remove_dir_all(&worktree_path);
    }
}

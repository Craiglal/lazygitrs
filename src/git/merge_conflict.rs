use std::collections::BTreeSet;
use std::process::Command;

use anyhow::{Context, Result, bail};

use super::GitCommands;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConflictStageContent {
    pub base: Option<Vec<u8>>,
    pub ours: Option<Vec<u8>>,
    pub theirs: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveChoice {
    Ours,
    Theirs,
    Both,
}

impl GitCommands {
    pub fn conflict_stages(&self, path: &str) -> Result<ConflictStageContent> {
        let stages = self.unmerged_stages_for_path(path)?;
        if stages.is_empty() {
            bail!("no unmerged entries found for {path}");
        }

        Ok(ConflictStageContent {
            base: if stages.contains(&1) {
                Some(self.read_index_stage(path, 1)?)
            } else {
                None
            },
            ours: if stages.contains(&2) {
                Some(self.read_index_stage(path, 2)?)
            } else {
                None
            },
            theirs: if stages.contains(&3) {
                Some(self.read_index_stage(path, 3)?)
            } else {
                None
            },
        })
    }

    pub fn has_unmerged_entries(&self, path: &str) -> Result<bool> {
        Ok(!self.unmerged_stages_for_path(path)?.is_empty())
    }

    pub fn unmerged_paths(&self) -> Result<Vec<String>> {
        let output = self
            .git()
            .args(&["ls-files", "-u"])
            .run_expecting_success()?;
        let mut paths = BTreeSet::new();
        for line in output.stdout.lines() {
            if let Some((_, path)) = line.split_once('\t') {
                paths.insert(path.to_string());
            }
        }
        Ok(paths.into_iter().collect())
    }

    pub fn resolve_conflict(&self, path: &str, choice: ResolveChoice) -> Result<()> {
        let stages = self.unmerged_stages_for_path(path)?;
        if stages.is_empty() {
            bail!("no unmerged entries found for {path}");
        }

        let needs_add = match choice {
            ResolveChoice::Ours => self.materialize_resolution_stage(path, 2, &stages)?,
            ResolveChoice::Theirs => self.materialize_resolution_stage(path, 3, &stages)?,
            ResolveChoice::Both => {
                if !stages.contains(&2) || !stages.contains(&3) {
                    bail!("cannot resolve {path} with both: one side deleted the file");
                }
                let contents = std::fs::read_to_string(self.repo_path().join(path))
                    .with_context(|| format!("failed to read conflicted file {path}"))?;
                let resolved = resolve_markers_with_both(&contents)?;
                self.write_worktree_file(path, resolved.as_bytes())?;
                true
            }
        };

        if needs_add {
            self.mark_conflict_resolved(path)
        } else {
            self.verify_no_unmerged_entries(path)
        }
    }

    pub fn mark_conflict_resolved(&self, path: &str) -> Result<()> {
        let full_path = self.repo_path().join(path);
        if let Ok(contents) = std::fs::read_to_string(&full_path)
            && contains_conflict_markers(&contents)
        {
            bail!("cannot mark {path} resolved while conflict markers remain");
        }

        self.stage_file(path)?;
        self.verify_no_unmerged_entries(path)
    }

    fn verify_no_unmerged_entries(&self, path: &str) -> Result<()> {
        if self.has_unmerged_entries(path)? {
            bail!("resolution did not clear unmerged entries for {path}");
        }
        Ok(())
    }

    fn unmerged_stages_for_path(&self, path: &str) -> Result<BTreeSet<u8>> {
        let output = self
            .git()
            .args(&["ls-files", "-u", "--", path])
            .run_expecting_success()?;
        let mut stages = BTreeSet::new();
        for line in output.stdout.lines() {
            if let Some(stage) = parse_unmerged_stage(line) {
                stages.insert(stage);
            }
        }
        Ok(stages)
    }

    fn read_index_stage(&self, path: &str, stage: u8) -> Result<Vec<u8>> {
        let spec = format!(":{stage}:{path}");
        let output = Command::new("git")
            .arg("show")
            .arg(&spec)
            .current_dir(self.repo_path())
            .output()
            .with_context(|| format!("failed to read stage {stage} for {path}"))?;
        if !output.status.success() {
            bail!(
                "failed to read stage {stage} for {path}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(output.stdout)
    }

    fn materialize_resolution_stage(
        &self,
        path: &str,
        stage: u8,
        stages: &BTreeSet<u8>,
    ) -> Result<bool> {
        if stages.contains(&stage) {
            self.git()
                .args(&[
                    "checkout-index",
                    "--force",
                    &format!("--stage={stage}"),
                    "--",
                    path,
                ])
                .run_expecting_success()
                .with_context(|| format!("failed to check out stage {stage} for {path}"))?;
            Ok(true)
        } else {
            self.git()
                .args(&["rm", "--force", "--", path])
                .run_expecting_success()
                .with_context(|| format!("failed to stage deletion for {path}"))?;
            Ok(false)
        }
    }

    fn write_worktree_file(&self, path: &str, contents: &[u8]) -> Result<()> {
        let full_path = self.repo_path().join(path);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create parent directory for {path}"))?;
        }
        std::fs::write(&full_path, contents)
            .with_context(|| format!("failed to write resolved file {path}"))?;
        Ok(())
    }
}

fn parse_unmerged_stage(line: &str) -> Option<u8> {
    let (meta, _) = line.split_once('\t')?;
    meta.split_whitespace().nth(2)?.parse().ok()
}

fn contains_conflict_markers(contents: &str) -> bool {
    contents.lines().any(|line| {
        line.starts_with("<<<<<<<")
            || line.starts_with("|||||||")
            || line.starts_with("=======")
            || line.starts_with(">>>>>>>")
    })
}

fn resolve_markers_with_both(contents: &str) -> Result<String> {
    let lines = split_inclusive_lines(contents);
    let mut output = String::new();
    let mut index = 0;
    let mut resolved_any = false;

    while index < lines.len() {
        let line = lines[index];
        if !line.starts_with("<<<<<<<") {
            output.push_str(line);
            index += 1;
            continue;
        }

        resolved_any = true;
        index += 1;
        let mut ours = String::new();
        let mut theirs = String::new();
        let mut saw_separator = false;
        let mut saw_end = false;

        while index < lines.len() {
            let line = lines[index];
            if line.starts_with("|||||||") {
                index += 1;
                while index < lines.len() && !lines[index].starts_with("=======") {
                    if lines[index].starts_with("<<<<<<<") || lines[index].starts_with(">>>>>>>") {
                        bail!("malformed conflict markers");
                    }
                    index += 1;
                }
                continue;
            }
            if line.starts_with("=======") {
                saw_separator = true;
                index += 1;
                break;
            }
            if line.starts_with(">>>>>>>") {
                bail!("malformed conflict markers");
            }
            ours.push_str(line);
            index += 1;
        }

        if !saw_separator {
            bail!("malformed conflict markers");
        }

        while index < lines.len() {
            let line = lines[index];
            if line.starts_with(">>>>>>>") {
                saw_end = true;
                index += 1;
                break;
            }
            if line.starts_with("<<<<<<<")
                || line.starts_with("|||||||")
                || line.starts_with("=======")
            {
                bail!("malformed conflict markers");
            }
            theirs.push_str(line);
            index += 1;
        }

        if !saw_end {
            bail!("malformed conflict markers");
        }

        output.push_str(&ours);
        output.push_str(&theirs);
    }

    if !resolved_any {
        bail!("no conflict markers found");
    }

    Ok(output)
}

fn split_inclusive_lines(contents: &str) -> Vec<&str> {
    if contents.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = contents.split_inclusive('\n').collect();
    let consumed: usize = lines.iter().map(|line| line.len()).sum();
    if consumed < contents.len() {
        lines.push(&contents[consumed..]);
    }
    lines
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::git::{GitCommands, merge_conflict::ResolveChoice};

    struct TestRepo {
        root: PathBuf,
    }

    impl TestRepo {
        fn with_text_conflict() -> Self {
            let root = unique_temp_dir("merge-conflict");
            init_repo(&root);

            fs::write(root.join("file.txt"), "before\nbase\nafter\n").unwrap();
            run_git(&root, &["add", "file.txt"]);
            run_git(&root, &["commit", "-m", "initial"]);
            let initial_branch = current_branch(&root);

            run_git(&root, &["checkout", "-b", "incoming"]);
            fs::write(root.join("file.txt"), "before\nincoming\nafter\n").unwrap();
            run_git(&root, &["commit", "-am", "incoming change"]);

            run_git(&root, &["checkout", &initial_branch]);
            fs::write(root.join("file.txt"), "before\ncurrent\nafter\n").unwrap();
            run_git(&root, &["commit", "-am", "current change"]);
            assert_conflicting_merge(&root, "incoming");

            Self { root }
        }

        fn with_delete_modify_conflict(deleted_on_current: bool) -> Self {
            let root = unique_temp_dir("merge-delete-modify");
            init_repo(&root);

            fs::write(root.join("file.txt"), "base\n").unwrap();
            run_git(&root, &["add", "file.txt"]);
            run_git(&root, &["commit", "-m", "initial"]);
            let initial_branch = current_branch(&root);

            run_git(&root, &["checkout", "-b", "incoming"]);
            if deleted_on_current {
                fs::write(root.join("file.txt"), "incoming\n").unwrap();
                run_git(&root, &["commit", "-am", "incoming modifies"]);
            } else {
                run_git(&root, &["rm", "file.txt"]);
                run_git(&root, &["commit", "-m", "incoming deletes"]);
            }

            run_git(&root, &["checkout", &initial_branch]);
            if deleted_on_current {
                run_git(&root, &["rm", "file.txt"]);
                run_git(&root, &["commit", "-m", "current deletes"]);
            } else {
                fs::write(root.join("file.txt"), "current\n").unwrap();
                run_git(&root, &["commit", "-am", "current modifies"]);
            }
            assert_conflicting_merge(&root, "incoming");

            Self { root }
        }

        #[cfg(unix)]
        fn with_executable_conflict() -> Self {
            use std::os::unix::fs::PermissionsExt;

            let root = unique_temp_dir("merge-executable");
            init_repo(&root);

            fs::write(root.join("script.sh"), "echo base\n").unwrap();
            run_git(&root, &["add", "script.sh"]);
            run_git(&root, &["commit", "-m", "initial"]);
            let initial_branch = current_branch(&root);

            run_git(&root, &["checkout", "-b", "incoming"]);
            fs::write(root.join("script.sh"), "echo incoming\n").unwrap();
            run_git(&root, &["commit", "-am", "incoming change"]);

            run_git(&root, &["checkout", &initial_branch]);
            fs::write(root.join("script.sh"), "echo current\n").unwrap();
            fs::set_permissions(root.join("script.sh"), fs::Permissions::from_mode(0o755)).unwrap();
            run_git(&root, &["add", "script.sh"]);
            run_git(&root, &["commit", "-m", "current executable change"]);
            assert_conflicting_merge(&root, "incoming");

            Self { root }
        }

        fn git(&self) -> GitCommands {
            GitCommands::new(&self.root).unwrap()
        }

        fn file_text(&self) -> String {
            self.file_text_at("file.txt")
        }

        fn file_text_at(&self, path: &str) -> String {
            fs::read_to_string(self.root.join(path)).unwrap()
        }
    }

    impl Drop for TestRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn loads_conflict_stages() {
        let repo = TestRepo::with_text_conflict();
        let stages = repo.git().conflict_stages("file.txt").unwrap();

        assert_eq!(
            stages.base.as_deref(),
            Some(b"before\nbase\nafter\n".as_slice())
        );
        assert_eq!(
            stages.ours.as_deref(),
            Some(b"before\ncurrent\nafter\n".as_slice())
        );
        assert_eq!(
            stages.theirs.as_deref(),
            Some(b"before\nincoming\nafter\n".as_slice())
        );
        assert!(repo.git().has_unmerged_entries("file.txt").unwrap());
        assert_eq!(repo.git().unmerged_paths().unwrap(), vec!["file.txt"]);
    }

    #[test]
    fn resolves_content_conflict_with_ours() {
        let repo = TestRepo::with_text_conflict();

        repo.git()
            .resolve_conflict("file.txt", ResolveChoice::Ours)
            .unwrap();

        assert_eq!(repo.file_text(), "before\ncurrent\nafter\n");
        assert!(!repo.git().has_unmerged_entries("file.txt").unwrap());
    }

    #[test]
    fn resolves_content_conflict_with_theirs() {
        let repo = TestRepo::with_text_conflict();

        repo.git()
            .resolve_conflict("file.txt", ResolveChoice::Theirs)
            .unwrap();

        assert_eq!(repo.file_text(), "before\nincoming\nafter\n");
        assert!(!repo.git().has_unmerged_entries("file.txt").unwrap());
    }

    #[test]
    fn resolves_content_conflict_with_both() {
        let repo = TestRepo::with_text_conflict();

        repo.git()
            .resolve_conflict("file.txt", ResolveChoice::Both)
            .unwrap();

        assert_eq!(repo.file_text(), "before\ncurrent\nincoming\nafter\n");
        assert!(!repo.file_text().contains("<<<<<<<"));
        assert!(!repo.git().has_unmerged_entries("file.txt").unwrap());
    }

    #[test]
    fn mark_resolved_rejects_remaining_markers() {
        let repo = TestRepo::with_text_conflict();

        let err = repo.git().mark_conflict_resolved("file.txt").unwrap_err();

        assert!(err.to_string().contains("conflict markers"));
        assert!(repo.git().has_unmerged_entries("file.txt").unwrap());
    }

    #[test]
    fn resolves_deleted_by_us_conflict_with_ours_deletion() {
        let repo = TestRepo::with_delete_modify_conflict(true);
        let stages = repo.git().conflict_stages("file.txt").unwrap();
        assert!(stages.ours.is_none());
        assert_eq!(stages.theirs.as_deref(), Some(b"incoming\n".as_slice()));

        repo.git()
            .resolve_conflict("file.txt", ResolveChoice::Ours)
            .unwrap();

        assert!(!repo.root.join("file.txt").exists());
        assert!(!repo.git().has_unmerged_entries("file.txt").unwrap());
    }

    #[test]
    fn resolves_deleted_by_them_conflict_with_theirs_deletion() {
        let repo = TestRepo::with_delete_modify_conflict(false);
        let stages = repo.git().conflict_stages("file.txt").unwrap();
        assert_eq!(stages.ours.as_deref(), Some(b"current\n".as_slice()));
        assert!(stages.theirs.is_none());

        repo.git()
            .resolve_conflict("file.txt", ResolveChoice::Theirs)
            .unwrap();

        assert!(!repo.root.join("file.txt").exists());
        assert!(!repo.git().has_unmerged_entries("file.txt").unwrap());
    }

    #[test]
    fn both_rejects_delete_modify_conflicts() {
        let repo = TestRepo::with_delete_modify_conflict(true);

        let err = repo
            .git()
            .resolve_conflict("file.txt", ResolveChoice::Both)
            .unwrap_err();

        assert!(err.to_string().contains("one side deleted"));
        assert!(repo.git().has_unmerged_entries("file.txt").unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn ours_resolution_preserves_executable_mode() {
        use std::os::unix::fs::PermissionsExt;

        let repo = TestRepo::with_executable_conflict();

        repo.git()
            .resolve_conflict("script.sh", ResolveChoice::Ours)
            .unwrap();

        let mode = fs::metadata(repo.root.join("script.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0);
        assert_eq!(repo.file_text_at("script.sh"), "echo current\n");
        assert!(!repo.git().has_unmerged_entries("script.sh").unwrap());
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "lazygitrs-{prefix}-{}-{unique}",
            std::process::id()
        ))
    }

    fn init_repo(root: &Path) {
        fs::create_dir_all(root).unwrap();
        run_git(root, &["init"]);
        run_git(root, &["config", "user.name", "Test User"]);
        run_git(root, &["config", "user.email", "test@example.com"]);
    }

    fn assert_conflicting_merge(root: &Path, branch: &str) {
        let merge = Command::new("git")
            .args(["merge", branch])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(!merge.status.success(), "merge unexpectedly succeeded");
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
}

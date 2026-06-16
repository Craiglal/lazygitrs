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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextConflictBlock {
    pub index: usize,
    /// Unchanged lines immediately before the changed region, for display only.
    pub context_before: String,
    pub base: Option<String>,
    pub ours: String,
    pub theirs: String,
    /// Unchanged lines immediately after the changed region, for display only.
    pub context_after: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TextConflictSegment {
    Context(String),
    Conflict(TextConflictBlock),
}

const CONFLICT_DISPLAY_CONTEXT_LINES: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
struct StageTextRegions {
    prefix: String,
    base: Option<String>,
    ours: String,
    theirs: String,
    suffix: String,
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

    pub fn conflict_blocks(&self, path: &str) -> Result<Vec<TextConflictBlock>> {
        if let Ok(contents) = std::fs::read_to_string(self.repo_path().join(path)) {
            let blocks = parse_text_conflict_blocks(&contents)?;
            if !blocks.is_empty() {
                return Ok(blocks);
            }
        }

        Ok(vec![self.stage_conflict_block(path)?])
    }

    pub fn resolve_conflict_blocks(&self, path: &str, choices: &[ResolveChoice]) -> Result<()> {
        let contents = std::fs::read_to_string(self.repo_path().join(path))
            .with_context(|| format!("failed to read conflicted file {path}"))?;
        let blocks = parse_text_conflict_blocks(&contents)?;
        if blocks.is_empty() {
            return self.resolve_stage_conflict_block(path, choices);
        }

        let resolved = resolve_text_conflict_blocks(&contents, choices)?;
        self.write_worktree_file(path, resolved.as_bytes())?;
        self.mark_conflict_resolved(path)
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

    fn stage_conflict_block(&self, path: &str) -> Result<TextConflictBlock> {
        let stages = self.conflict_stages(path)?;
        let stage_text = stage_text_regions(stages)
            .with_context(|| format!("failed to build stage-backed conflict block for {path}"))?;
        if stage_text.ours.is_empty() && stage_text.theirs.is_empty() {
            bail!("no text conflict content found for {path}");
        }

        Ok(TextConflictBlock {
            index: 0,
            context_before: tail_lines(&stage_text.prefix, CONFLICT_DISPLAY_CONTEXT_LINES),
            base: stage_text.base,
            ours: stage_text.ours,
            theirs: stage_text.theirs,
            context_after: head_lines(&stage_text.suffix, CONFLICT_DISPLAY_CONTEXT_LINES),
        })
    }

    fn resolve_stage_conflict_block(&self, path: &str, choices: &[ResolveChoice]) -> Result<()> {
        let [choice] = choices else {
            bail!("choice count mismatch: got {}, expected 1", choices.len());
        };

        match choice {
            ResolveChoice::Ours => self.resolve_conflict(path, ResolveChoice::Ours),
            ResolveChoice::Theirs => self.resolve_conflict(path, ResolveChoice::Theirs),
            ResolveChoice::Both => {
                let stages = self.conflict_stages(path)?;
                let resolved = resolve_stage_text_regions(stages)?;
                self.write_worktree_file(path, resolved.as_bytes())?;
                self.mark_conflict_resolved(path)
            }
        }
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

fn stage_text_regions(stages: ConflictStageContent) -> Result<StageTextRegions> {
    let base = stages
        .base
        .map(String::from_utf8)
        .transpose()
        .context("base stage is not UTF-8 text")?;
    let ours = stages
        .ours
        .map(String::from_utf8)
        .transpose()
        .context("ours stage is not UTF-8 text")?
        .unwrap_or_default();
    let theirs = stages
        .theirs
        .map(String::from_utf8)
        .transpose()
        .context("theirs stage is not UTF-8 text")?
        .unwrap_or_default();

    Ok(match base {
        Some(base) if !ours.is_empty() && !theirs.is_empty() => {
            split_stage_text_regions(&base, &ours, &theirs)
        }
        base => StageTextRegions {
            prefix: String::new(),
            base,
            ours,
            theirs,
            suffix: String::new(),
        },
    })
}

fn resolve_stage_text_regions(stages: ConflictStageContent) -> Result<String> {
    let regions = stage_text_regions(stages)?;
    let mut resolved = String::new();
    resolved.push_str(&regions.prefix);
    resolved.push_str(&regions.ours);
    resolved.push_str(&regions.theirs);
    resolved.push_str(&regions.suffix);
    Ok(resolved)
}

fn split_stage_text_regions(base: &str, ours: &str, theirs: &str) -> StageTextRegions {
    let base_lines = split_inclusive_lines(base);
    let ours_lines = split_inclusive_lines(ours);
    let theirs_lines = split_inclusive_lines(theirs);

    let min_len = base_lines
        .len()
        .min(ours_lines.len())
        .min(theirs_lines.len());
    let mut prefix_len = 0;
    while prefix_len < min_len
        && base_lines[prefix_len] == ours_lines[prefix_len]
        && base_lines[prefix_len] == theirs_lines[prefix_len]
    {
        prefix_len += 1;
    }

    let mut suffix_len = 0;
    while suffix_len < min_len.saturating_sub(prefix_len)
        && base_lines[base_lines.len() - 1 - suffix_len]
            == ours_lines[ours_lines.len() - 1 - suffix_len]
        && base_lines[base_lines.len() - 1 - suffix_len]
            == theirs_lines[theirs_lines.len() - 1 - suffix_len]
    {
        suffix_len += 1;
    }

    let base_mid_end = base_lines.len().saturating_sub(suffix_len);
    let ours_mid_end = ours_lines.len().saturating_sub(suffix_len);
    let theirs_mid_end = theirs_lines.len().saturating_sub(suffix_len);

    StageTextRegions {
        prefix: base_lines[..prefix_len].concat(),
        base: Some(base_lines[prefix_len..base_mid_end].concat()),
        ours: ours_lines[prefix_len..ours_mid_end].concat(),
        theirs: theirs_lines[prefix_len..theirs_mid_end].concat(),
        suffix: base_lines[base_mid_end..].concat(),
    }
}

fn contains_conflict_markers(contents: &str) -> bool {
    contents.lines().any(|line| {
        line.starts_with("<<<<<<<")
            || line.starts_with("|||||||")
            || line.starts_with("=======")
            || line.starts_with(">>>>>>>")
    })
}

fn blocks_with_display_context(segments: &[TextConflictSegment]) -> Vec<TextConflictBlock> {
    segments
        .iter()
        .enumerate()
        .filter_map(|(idx, segment)| match segment {
            TextConflictSegment::Context(_) => None,
            TextConflictSegment::Conflict(block) => {
                let context_before = match idx.checked_sub(1).and_then(|prev| segments.get(prev)) {
                    Some(TextConflictSegment::Context(context)) => {
                        tail_lines(context, CONFLICT_DISPLAY_CONTEXT_LINES)
                    }
                    _ => String::new(),
                };
                let context_after = match segments.get(idx + 1) {
                    Some(TextConflictSegment::Context(context)) => {
                        head_lines(context, CONFLICT_DISPLAY_CONTEXT_LINES)
                    }
                    _ => String::new(),
                };
                let mut block = block.clone();
                block.context_before = context_before;
                block.context_after = context_after;
                Some(block)
            }
        })
        .collect()
}

fn head_lines(contents: &str, limit: usize) -> String {
    split_inclusive_lines(contents)
        .into_iter()
        .take(limit)
        .collect()
}

fn tail_lines(contents: &str, limit: usize) -> String {
    let lines = split_inclusive_lines(contents);
    let start = lines.len().saturating_sub(limit);
    lines.into_iter().skip(start).collect()
}

pub fn parse_text_conflict_blocks(contents: &str) -> Result<Vec<TextConflictBlock>> {
    let segments = parse_text_conflict_segments(contents)?;
    Ok(blocks_with_display_context(&segments))
}

pub fn resolve_text_conflict_blocks(contents: &str, choices: &[ResolveChoice]) -> Result<String> {
    let segments = parse_text_conflict_segments(contents)?;
    let block_count = segments
        .iter()
        .filter(|segment| matches!(segment, TextConflictSegment::Conflict(_)))
        .count();
    if block_count == 0 {
        bail!("no conflict markers found");
    }
    if choices.len() != block_count {
        bail!(
            "choice count mismatch: got {}, expected {block_count}",
            choices.len()
        );
    }

    let mut output = String::new();
    let mut choice_idx = 0;
    for segment in segments {
        match segment {
            TextConflictSegment::Context(context) => output.push_str(&context),
            TextConflictSegment::Conflict(block) => {
                match choices[choice_idx] {
                    ResolveChoice::Ours => output.push_str(&block.ours),
                    ResolveChoice::Theirs => output.push_str(&block.theirs),
                    ResolveChoice::Both => {
                        output.push_str(&block.ours);
                        output.push_str(&block.theirs);
                    }
                }
                choice_idx += 1;
            }
        }
    }

    Ok(output)
}

fn parse_text_conflict_segments(contents: &str) -> Result<Vec<TextConflictSegment>> {
    let lines = split_inclusive_lines(contents);
    let mut segments = Vec::new();
    let mut context = String::new();
    let mut index = 0;
    let mut block_index = 0;

    while index < lines.len() {
        let line = lines[index];
        if !line.starts_with("<<<<<<<") {
            context.push_str(line);
            index += 1;
            continue;
        }

        if !context.is_empty() {
            segments.push(TextConflictSegment::Context(std::mem::take(&mut context)));
        }

        index += 1;
        let mut ours = String::new();
        let mut base: Option<String> = None;
        let mut theirs = String::new();
        let mut saw_separator = false;
        let mut saw_end = false;

        while index < lines.len() {
            let line = lines[index];
            if line.starts_with("|||||||") {
                index += 1;
                let mut base_content = String::new();
                while index < lines.len() && !lines[index].starts_with("=======") {
                    if lines[index].starts_with("<<<<<<<") || lines[index].starts_with(">>>>>>>") {
                        bail!("malformed conflict markers");
                    }
                    base_content.push_str(lines[index]);
                    index += 1;
                }
                base = Some(base_content);
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

        segments.push(TextConflictSegment::Conflict(TextConflictBlock {
            index: block_index,
            context_before: String::new(),
            base,
            ours,
            theirs,
            context_after: String::new(),
        }));
        block_index += 1;
    }

    if !context.is_empty() {
        segments.push(TextConflictSegment::Context(context));
    }

    Ok(segments)
}

fn resolve_markers_with_both(contents: &str) -> Result<String> {
    let block_count = parse_text_conflict_blocks(contents)?.len();
    resolve_text_conflict_blocks(contents, &vec![ResolveChoice::Both; block_count])
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

        fn with_two_text_conflicts() -> Self {
            let root = unique_temp_dir("merge-multi-conflict");
            init_repo(&root);

            fs::write(root.join("file.txt"), "a\nbase1\nb\nbase2\nc\n").unwrap();
            run_git(&root, &["add", "file.txt"]);
            run_git(&root, &["commit", "-m", "initial"]);
            let initial_branch = current_branch(&root);

            run_git(&root, &["checkout", "-b", "incoming"]);
            fs::write(root.join("file.txt"), "a\nincoming1\nb\nincoming2\nc\n").unwrap();
            run_git(&root, &["commit", "-am", "incoming changes"]);

            run_git(&root, &["checkout", &initial_branch]);
            fs::write(root.join("file.txt"), "a\ncurrent1\nb\ncurrent2\nc\n").unwrap();
            run_git(&root, &["commit", "-am", "current changes"]);
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
    fn parses_diff3_conflict_blocks_with_base_sections() {
        let blocks = super::parse_text_conflict_blocks(
            "start\n<<<<<<< ours\ncurrent\n||||||| base\nbase\n=======\nincoming\n>>>>>>> theirs\nend\n",
        )
        .unwrap();

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].ours, "current\n");
        assert_eq!(blocks[0].base.as_deref(), Some("base\n"));
        assert_eq!(blocks[0].theirs, "incoming\n");
    }

    #[test]
    fn conflict_blocks_include_nearby_context_for_display() {
        let blocks = super::parse_text_conflict_blocks(
            "keep-1\nkeep-2\nkeep-3\nkeep-4\n<<<<<<< ours\ncurrent\n=======\nincoming\n>>>>>>> theirs\nafter-1\nafter-2\nafter-3\nafter-4\n",
        )
        .unwrap();

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].context_before, "keep-2\nkeep-3\nkeep-4\n");
        assert_eq!(blocks[0].context_after, "after-1\nafter-2\nafter-3\n");
    }

    #[test]
    fn resolves_conflict_blocks_with_independent_choices() {
        let resolved = super::resolve_text_conflict_blocks(
            "a\n<<<<<<< ours\no1\n=======\nt1\n>>>>>>> theirs\nb\n<<<<<<< ours\no2\n=======\nt2\n>>>>>>> theirs\nc\n",
            &[ResolveChoice::Theirs, ResolveChoice::Both],
        )
        .unwrap();

        assert_eq!(resolved, "a\nt1\nb\no2\nt2\nc\n");
    }

    #[test]
    fn per_block_resolution_rejects_choice_count_mismatch() {
        let err = super::resolve_text_conflict_blocks(
            "<<<<<<< ours\no\n=======\nt\n>>>>>>> theirs\n",
            &[],
        )
        .unwrap_err();

        assert!(err.to_string().contains("choice count"));
    }

    #[test]
    fn resolves_real_file_with_per_block_choices() {
        let repo = TestRepo::with_two_text_conflicts();
        let blocks = super::parse_text_conflict_blocks(&repo.file_text()).unwrap();
        assert_eq!(blocks.len(), 2);

        repo.git()
            .resolve_conflict_blocks("file.txt", &[ResolveChoice::Theirs, ResolveChoice::Both])
            .unwrap();

        assert_eq!(
            repo.file_text(),
            "a\nincoming1\nb\ncurrent2\nincoming2\nc\n"
        );
        assert!(!repo.git().has_unmerged_entries("file.txt").unwrap());
    }

    #[test]
    fn conflict_blocks_fall_back_to_stages_when_worktree_has_no_markers() {
        let repo = TestRepo::with_text_conflict();
        fs::write(repo.root.join("file.txt"), "manual edit without markers\n").unwrap();

        let blocks = repo.git().conflict_blocks("file.txt").unwrap();

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].context_before, "before\n");
        assert_eq!(blocks[0].base.as_deref(), Some("base\n"));
        assert_eq!(blocks[0].ours, "current\n");
        assert_eq!(blocks[0].theirs, "incoming\n");
        assert_eq!(blocks[0].context_after, "after\n");
    }

    #[test]
    fn resolves_stage_fallback_block_when_worktree_has_no_markers() {
        let repo = TestRepo::with_text_conflict();
        fs::write(repo.root.join("file.txt"), "manual edit without markers\n").unwrap();

        repo.git()
            .resolve_conflict_blocks("file.txt", &[ResolveChoice::Theirs])
            .unwrap();

        assert_eq!(repo.file_text(), "before\nincoming\nafter\n");
        assert!(!repo.git().has_unmerged_entries("file.txt").unwrap());
    }

    #[test]
    fn stage_fallback_both_keeps_shared_context_once() {
        let repo = TestRepo::with_text_conflict();
        fs::write(repo.root.join("file.txt"), "manual edit without markers\n").unwrap();

        repo.git()
            .resolve_conflict_blocks("file.txt", &[ResolveChoice::Both])
            .unwrap();

        assert_eq!(repo.file_text(), "before\ncurrent\nincoming\nafter\n");
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

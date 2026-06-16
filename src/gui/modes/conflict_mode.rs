use anyhow::{Result, bail};

use crate::git::merge_conflict::{ResolveChoice, TextConflictBlock};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictBlockState {
    pub block: TextConflictBlock,
    pub choice: Option<ResolveChoice>,
}

#[derive(Debug, Clone)]
pub struct ConflictModeState {
    pub active: bool,
    pub path: String,
    pub blocks: Vec<ConflictBlockState>,
    pub selected: usize,
    pub scroll: usize,
    pub visible_height: usize,
}

impl ConflictModeState {
    pub fn new() -> Self {
        Self {
            active: false,
            path: String::new(),
            blocks: Vec::new(),
            selected: 0,
            scroll: 0,
            visible_height: 0,
        }
    }

    pub fn enter(&mut self, path: String, blocks: Vec<TextConflictBlock>) {
        self.active = true;
        self.path = path;
        self.blocks = blocks
            .into_iter()
            .map(|block| ConflictBlockState {
                block,
                choice: None,
            })
            .collect();
        self.selected = 0;
        self.scroll = 0;
        self.visible_height = 0;
    }

    pub fn exit(&mut self) {
        self.active = false;
        self.path.clear();
        self.blocks.clear();
        self.selected = 0;
        self.scroll = 0;
        self.visible_height = 0;
    }

    pub fn set_choice_current(&mut self, choice: ResolveChoice) {
        if let Some(block) = self.blocks.get_mut(self.selected) {
            block.choice = Some(choice);
        }
    }

    pub fn unresolved_count(&self) -> usize {
        self.blocks
            .iter()
            .filter(|block| block.choice.is_none())
            .count()
    }

    pub fn collect_choices(&self) -> Result<Vec<ResolveChoice>> {
        if let Some(first_unresolved) = self.blocks.iter().position(|block| block.choice.is_none())
        {
            bail!(
                "Choose ours/theirs/both for every block first (block {} is unresolved)",
                first_unresolved + 1
            );
        }
        Ok(self
            .blocks
            .iter()
            .filter_map(|block| block.choice)
            .collect())
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
        self.ensure_visible(self.visible_height);
    }

    pub fn move_down(&mut self) {
        if self.selected + 1 < self.blocks.len() {
            self.selected += 1;
        }
        self.ensure_visible(self.visible_height);
    }

    pub fn ensure_visible(&mut self, visible_height: usize) {
        self.visible_height = visible_height;
        let visible_height = visible_height.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + visible_height {
            self.scroll = self.selected.saturating_sub(visible_height - 1);
        }
        let max_scroll = self.blocks.len().saturating_sub(visible_height);
        self.scroll = self.scroll.min(max_scroll);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(index: usize) -> TextConflictBlock {
        TextConflictBlock {
            index,
            context_before: String::new(),
            base: Some(format!("base-{index}\n")),
            ours: format!("ours-{index}\n"),
            theirs: format!("theirs-{index}\n"),
            context_after: String::new(),
        }
    }

    #[test]
    fn enter_starts_with_all_blocks_unresolved() {
        let mut state = ConflictModeState::new();

        state.enter("file.txt".to_string(), vec![block(0), block(1)]);

        assert!(state.active);
        assert_eq!(state.path, "file.txt");
        assert_eq!(state.selected, 0);
        assert_eq!(state.scroll, 0);
        assert_eq!(state.blocks.len(), 2);
        assert!(state.blocks.iter().all(|block| block.choice.is_none()));
        assert_eq!(state.unresolved_count(), 2);
    }

    #[test]
    fn collect_choices_rejects_unresolved_blocks() {
        let mut state = ConflictModeState::new();
        state.enter("file.txt".to_string(), vec![block(0), block(1)]);
        state.set_choice_current(ResolveChoice::Ours);

        let err = state.collect_choices().unwrap_err();

        assert!(err.to_string().contains("Choose ours/theirs/both"));
        assert_eq!(state.unresolved_count(), 1);
    }

    #[test]
    fn collect_choices_returns_ordered_choices_after_all_blocks_resolved() {
        let mut state = ConflictModeState::new();
        state.enter("file.txt".to_string(), vec![block(0), block(1)]);
        state.set_choice_current(ResolveChoice::Theirs);
        state.move_down();
        state.set_choice_current(ResolveChoice::Both);

        assert_eq!(
            state.collect_choices().unwrap(),
            vec![ResolveChoice::Theirs, ResolveChoice::Both]
        );
        assert_eq!(state.unresolved_count(), 0);
    }

    #[test]
    fn navigation_is_bounded_and_keeps_selection_visible() {
        let mut state = ConflictModeState::new();
        state.enter(
            "file.txt".to_string(),
            vec![block(0), block(1), block(2), block(3)],
        );
        state.visible_height = 2;

        state.move_up();
        assert_eq!(state.selected, 0);

        state.move_down();
        state.move_down();
        state.move_down();
        state.move_down();

        assert_eq!(state.selected, 3);
        assert_eq!(state.scroll, 2);
    }
}

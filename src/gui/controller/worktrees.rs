use std::collections::HashSet;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};

use crate::config::KeybindingConfig;
use crate::gui::Gui;
use crate::gui::controller::input_normalization::replace_spaces_with_dashes;
use crate::gui::popup::{
    ListPickerCore, ListPickerItem, MenuItem, PopupState, make_help_search_textarea, make_textarea,
};

pub fn handle_key(gui: &mut Gui, key: KeyEvent, _keybindings: &KeybindingConfig) -> Result<()> {
    // Switch to worktree
    if key.code == KeyCode::Char(' ') {
        return switch_worktree(gui);
    }

    // Create new worktree
    if key.code == KeyCode::Char('n') {
        return create_worktree(gui);
    }

    // Remove worktree
    if key.code == KeyCode::Char('d') {
        return remove_worktree(gui);
    }

    Ok(())
}

fn switch_worktree(gui: &mut Gui) -> Result<()> {
    let selected = gui.context_mgr.selected_active();
    let model = gui.model.lock().unwrap();
    if let Some(wt) = model.worktrees.get(selected) {
        if wt.is_current {
            return Ok(()); // Already in this worktree
        }
        let path = wt.path.clone();
        let branch = wt.branch.clone();
        drop(model);

        gui.popup = PopupState::Confirm {
            title: "Switch worktree".to_string(),
            message: format!(
                "Open lazygitrs in worktree '{}' ({})?\nThis will launch a new instance.",
                branch, path
            ),
            on_confirm: Box::new(move |gui| {
                gui.request_repo_open(path.clone());
                Ok(())
            }),
        };
    }
    Ok(())
}

fn create_worktree(gui: &mut Gui) -> Result<()> {
    prompt_worktree_path(gui);
    Ok(())
}

fn prompt_worktree_path(gui: &mut Gui) {
    gui.popup = PopupState::Input {
        title: "New worktree path".to_string(),
        textarea: make_textarea("Path"),
        on_confirm: Box::new(|gui, input| {
            let path = replace_spaces_with_dashes(input);
            if path.is_empty() {
                prompt_worktree_path(gui);
                return Ok(());
            }
            prompt_worktree_creation_mode(gui, path);
            Ok(())
        }),
        is_commit: false,
        confirm_focused: false,
    };
}

fn prompt_worktree_creation_mode(gui: &mut Gui, path: String) {
    let existing_branch_items = worktree_existing_local_branch_items(gui);
    if existing_branch_items.is_empty() {
        prompt_worktree_branch(gui, path);
        return;
    }

    let existing_path = path.clone();
    let new_path = path;
    gui.popup = PopupState::Menu {
        title: "Create worktree".to_string(),
        items: vec![
            MenuItem {
                label: "Existing local branch".to_string(),
                description: "Create a worktree from an available local branch".to_string(),
                key: Some("e".to_string()),
                action: Some(Box::new(move |gui| {
                    prompt_worktree_existing_branch(gui, existing_path.clone());
                    Ok(())
                })),
            },
            MenuItem {
                label: "New branch".to_string(),
                description: "Create a new branch for the worktree".to_string(),
                key: Some("n".to_string()),
                action: Some(Box::new(move |gui| {
                    prompt_worktree_branch(gui, new_path.clone());
                    Ok(())
                })),
            },
        ],
        selected: 0,
        loading_index: None,
    };
}

fn prompt_worktree_existing_branch(gui: &mut Gui, path: String) {
    gui.popup = PopupState::RefPicker {
        title: "Existing local branch for worktree".to_string(),
        core: ListPickerCore {
            items: worktree_existing_local_branch_items(gui),
            selected: 0,
            search_textarea: make_help_search_textarea(),
            scroll_offset: 0,
        },
        allow_freeform: false,
        on_confirm: Box::new(move |gui, branch| {
            if !is_allowed_existing_local_branch(branch, &worktree_existing_local_branch_items(gui))
            {
                anyhow::bail!("Select an existing local branch from the list")
            }
            gui.git.create_worktree_existing_branch(&path, branch)?;
            gui.needs_refresh = true;
            Ok(())
        }),
    };
}

fn prompt_worktree_branch(gui: &mut Gui, path: String) {
    let mut textarea = make_textarea("Branch name");
    if let Some(branch_name) = default_branch_name_from_path(&path) {
        textarea.insert_str(&branch_name);
    }

    gui.popup = PopupState::Input {
        title: "New worktree branch".to_string(),
        textarea,
        on_confirm: Box::new(move |gui, input| {
            let branch = replace_spaces_with_dashes(input);
            if branch.is_empty() {
                prompt_worktree_branch(gui, path);
                return Ok(());
            }
            prompt_worktree_base_ref(gui, path, branch);
            Ok(())
        }),
        is_commit: false,
        confirm_focused: false,
    };
}

fn prompt_worktree_base_ref(gui: &mut Gui, path: String, branch: String) {
    gui.popup = PopupState::RefPicker {
        title: "Base ref for worktree".to_string(),
        core: ListPickerCore {
            items: worktree_base_ref_items(gui),
            selected: 0,
            search_textarea: make_help_search_textarea(),
            scroll_offset: 0,
        },
        allow_freeform: true,
        on_confirm: Box::new(move |gui, base_ref| {
            gui.git
                .create_worktree_new_branch(&path, &branch, Some(base_ref))?;
            gui.needs_refresh = true;
            Ok(())
        }),
    };
}

fn default_branch_name_from_path(path: &str) -> Option<String> {
    std::path::Path::new(path)
        .file_name()
        .and_then(|name| sanitize_branch_name_suggestion(&name.to_string_lossy()))
}

fn sanitize_branch_name_suggestion(raw: &str) -> Option<String> {
    let mut sanitized = String::new();
    let mut last_was_dash = false;

    for ch in raw.trim().chars() {
        let mapped = if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            Some(ch)
        } else {
            Some('-')
        };

        if let Some(ch) = mapped {
            if ch == '-' {
                if sanitized.is_empty() || last_was_dash {
                    continue;
                }
                last_was_dash = true;
            } else {
                last_was_dash = false;
            }
            sanitized.push(ch);
        }
    }

    let sanitized = sanitized
        .trim_matches(|ch| matches!(ch, '-' | '.'))
        .to_string();

    if sanitized.is_empty() {
        None
    } else {
        Some(sanitized)
    }
}

fn worktree_existing_local_branch_items(gui: &Gui) -> Vec<ListPickerItem> {
    let model = gui.model.lock().unwrap();
    existing_local_branch_items(&model.branches, &model.worktrees)
}

fn existing_local_branch_items(
    branches: &[crate::model::Branch],
    worktrees: &[crate::model::Worktree],
) -> Vec<ListPickerItem> {
    let occupied_branches: HashSet<&str> = worktrees
        .iter()
        .map(|worktree| worktree.branch.trim())
        .filter(|branch| !branch.is_empty() && *branch != "(detached)")
        .collect();

    branches
        .iter()
        .map(|branch| branch.name.trim())
        .filter(|branch| !branch.is_empty() && !occupied_branches.contains(branch))
        .map(|branch| ListPickerItem {
            value: branch.to_string(),
            label: branch.to_string(),
            category: "Local Branches".to_string(),
        })
        .collect()
}

fn is_allowed_existing_local_branch(branch: &str, items: &[ListPickerItem]) -> bool {
    let branch = branch.trim();
    !branch.is_empty() && items.iter().any(|item| item.value == branch)
}

fn worktree_base_ref_items(gui: &Gui) -> Vec<ListPickerItem> {
    let model = gui.model.lock().unwrap();
    let mut items = Vec::new();
    let current_ref = if !model.head_branch_name.trim().is_empty() {
        Some((
            "Current Branch".to_string(),
            format!("{} (current, default)", model.head_branch_name),
        ))
    } else if !model.head_hash.trim().is_empty() {
        Some((
            "Current HEAD".to_string(),
            format!("{} (current HEAD, default)", model.head_hash),
        ))
    } else {
        None
    };

    if let Some((category, label)) = current_ref {
        items.push(ListPickerItem {
            value: String::new(),
            label,
            category,
        });
    }

    for branch in &model.branches {
        if branch.head {
            continue;
        }
        items.push(ListPickerItem {
            value: branch.name.clone(),
            label: branch.name.clone(),
            category: "Branches".to_string(),
        });
    }

    for remote in &model.remotes {
        for branch in &remote.branches {
            let full_name = format!("{}/{}", remote.name, branch.name);
            items.push(ListPickerItem {
                value: full_name.clone(),
                label: full_name,
                category: "Remote Branches".to_string(),
            });
        }
    }

    for tag in &model.tags {
        items.push(ListPickerItem {
            value: tag.name.clone(),
            label: tag.name.clone(),
            category: "Tags".to_string(),
        });
    }

    for commit in &model.commits {
        items.push(ListPickerItem {
            value: commit.hash.clone(),
            label: format!("{} {}", commit.short_hash(), commit.name),
            category: "Commits".to_string(),
        });
    }

    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Branch, Worktree};

    fn branch(name: &str, head: bool) -> Branch {
        Branch {
            name: name.to_string(),
            hash: String::new(),
            recency: String::new(),
            pushables: String::new(),
            pullables: String::new(),
            upstream: None,
            head,
        }
    }

    fn worktree(branch: &str) -> Worktree {
        Worktree {
            path: String::new(),
            branch: branch.to_string(),
            hash: String::new(),
            is_current: false,
            is_main: false,
        }
    }

    #[test]
    fn existing_local_branch_items_exclude_occupied_and_detached_worktree_markers() {
        let branches = vec![
            branch("main", true),
            branch("feature", false),
            branch("occupied", false),
            branch("detached-name", false),
            branch("", false),
        ];
        let worktrees = vec![
            worktree("main"),
            worktree("occupied"),
            worktree(""),
            worktree("(detached)"),
        ];

        let items = existing_local_branch_items(&branches, &worktrees);

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].value, "feature");
        assert_eq!(items[0].label, "feature");
        assert_eq!(items[0].category, "Local Branches");
        assert_eq!(items[1].value, "detached-name");
    }

    #[test]
    fn existing_local_branch_validation_rejects_raw_non_listed_refs() {
        let items = vec![ListPickerItem {
            value: "feature".to_string(),
            label: "feature".to_string(),
            category: "Local Branches".to_string(),
        }];

        assert!(is_allowed_existing_local_branch("feature", &items));
        assert!(is_allowed_existing_local_branch(" feature ", &items));
        assert!(!is_allowed_existing_local_branch("origin/feature", &items));
        assert!(!is_allowed_existing_local_branch("deadbeef", &items));
        assert!(!is_allowed_existing_local_branch("", &items));
    }

    #[test]
    fn default_branch_name_from_path_sanitizes_space_containing_paths() {
        assert_eq!(
            default_branch_name_from_path("/tmp/feature branch"),
            Some("feature-branch".to_string())
        );
        assert_eq!(
            default_branch_name_from_path("/tmp/  spaced   branch  "),
            Some("spaced-branch".to_string())
        );
        assert_eq!(default_branch_name_from_path("/tmp/###"), None);
    }
}

fn remove_worktree(gui: &mut Gui) -> Result<()> {
    let selected = gui.context_mgr.selected_active();
    let model = gui.model.lock().unwrap();
    if let Some(wt) = model.worktrees.get(selected) {
        if wt.is_current || wt.is_main {
            return Ok(()); // Can't remove current or main worktree
        }
        let path = wt.path.clone();
        let branch = wt.branch.clone();
        drop(model);

        gui.popup = PopupState::Confirm {
            title: "Remove worktree".to_string(),
            message: format!(
                "Remove worktree '{}' ({})?\nThis won't delete the branch.",
                branch, path
            ),
            on_confirm: Box::new(move |gui| {
                gui.git.remove_worktree(&path, false)?;
                gui.needs_refresh = true;
                Ok(())
            }),
        };
    }
    Ok(())
}

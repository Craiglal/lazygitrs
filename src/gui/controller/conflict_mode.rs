use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::git::merge_conflict::ResolveChoice;
use crate::gui::Gui;
use crate::gui::popup::{MessageKind, PopupState};

pub fn handle_key(gui: &mut Gui, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => {
            gui.conflict_mode.exit();
            Ok(())
        }
        KeyCode::Char('j') | KeyCode::Down if !key.modifiers.contains(KeyModifiers::ALT) => {
            gui.conflict_mode.move_down();
            Ok(())
        }
        KeyCode::Char('k') | KeyCode::Up if !key.modifiers.contains(KeyModifiers::ALT) => {
            gui.conflict_mode.move_up();
            Ok(())
        }
        KeyCode::Char('n') => {
            move_to_next_unresolved(gui);
            Ok(())
        }
        KeyCode::Char('p') => {
            move_to_prev_unresolved(gui);
            Ok(())
        }
        KeyCode::Char('o') => {
            gui.conflict_mode.set_choice_current(ResolveChoice::Ours);
            Ok(())
        }
        KeyCode::Char('t') => {
            gui.conflict_mode.set_choice_current(ResolveChoice::Theirs);
            Ok(())
        }
        KeyCode::Char('b') => {
            gui.conflict_mode.set_choice_current(ResolveChoice::Both);
            Ok(())
        }
        KeyCode::Enter | KeyCode::Char('s') => save(gui),
        _ => Ok(()),
    }
}

fn save(gui: &mut Gui) -> Result<()> {
    let choices = match gui.conflict_mode.collect_choices() {
        Ok(choices) => choices,
        Err(err) => {
            gui.popup = PopupState::Message {
                title: "Unresolved conflict blocks".to_string(),
                message: format!("{err}"),
                kind: MessageKind::Error,
            };
            return Ok(());
        }
    };
    let path = gui.conflict_mode.path.clone();
    if let Err(err) = gui.git.resolve_conflict_blocks(&path, &choices) {
        gui.popup = PopupState::Message {
            title: "Resolve conflict failed".to_string(),
            message: format!("{err:#}"),
            kind: MessageKind::Error,
        };
        return Ok(());
    }

    gui.conflict_mode.exit();
    gui.needs_refresh = true;
    gui.needs_files_refresh = true;
    gui.needs_diff_refresh = true;
    Ok(())
}

fn move_to_next_unresolved(gui: &mut Gui) {
    if gui.conflict_mode.blocks.is_empty() {
        return;
    }
    let len = gui.conflict_mode.blocks.len();
    for step in 1..=len {
        let idx = (gui.conflict_mode.selected + step) % len;
        if gui.conflict_mode.blocks[idx].choice.is_none() {
            gui.conflict_mode.selected = idx;
            gui.conflict_mode
                .ensure_visible(gui.conflict_mode.visible_height);
            return;
        }
    }
}

fn move_to_prev_unresolved(gui: &mut Gui) {
    if gui.conflict_mode.blocks.is_empty() {
        return;
    }
    let len = gui.conflict_mode.blocks.len();
    for step in 1..=len {
        let idx = (gui.conflict_mode.selected + len - step) % len;
        if gui.conflict_mode.blocks[idx].choice.is_none() {
            gui.conflict_mode.selected = idx;
            gui.conflict_mode
                .ensure_visible(gui.conflict_mode.visible_height);
            return;
        }
    }
}

//! Actions that need the real terminal: they are requested by key handlers as
//! plain data, then executed by the main loop, which owns the `Terminal`.

use std::panic::{self, AssertUnwindSafe};
use std::process::Command;

use anyhow::{Result, bail};

use crate::config::user_config::OsConfig;
use crate::os::editor::{self, EditorCmd};

use super::{Term, enter_terminal_modes, restore_terminal};

/// A request to open a file in the user's editor.
pub struct EditRequest {
    /// Absolute path to a file the call site has already verified exists.
    pub path: String,
    pub line: Option<usize>,
    /// 1-based column; `1` when the call site has no column information.
    pub column: usize,
}

impl EditRequest {
    /// Build a request with no column information.
    pub fn at(path: String, line: Option<usize>) -> Self {
        Self {
            path,
            line,
            column: 1,
        }
    }
}

/// An action deferred to the main loop because it needs the terminal.
pub enum Interactive {
    Edit(EditRequest),
    // A `Review(..)` variant lands here with the tuicr <c-r> integration.
}

/// Hand the terminal to `f`, then restore it on a normal return.
///
/// The returned `Result` reports only whether the *terminal* survived. `f`'s own
/// outcome comes back as `R`, so a caller can tell "the editor failed" (worth a
/// popup) from "the terminal is gone" (fatal).
///
/// If `f` panics, the terminal is deliberately **not** rebuilt: it is already
/// restored (we did that above, before calling `f`), and `main.rs`'s panic
/// hook runs at the panic site — before `catch_unwind` returns — so by the
/// time we get control back the panic message has already been printed to
/// the primary screen. Rebuilding here would re-enter raw mode and the
/// alternate screen with no `Drop` guard left on the unwind path to undo it,
/// leaving the user's shell stuck in a wrecked terminal after the process
/// exits.
pub fn run_with_terminal_suspended<F, R>(
    terminal: &mut Term,
    keyboard_enhanced: bool,
    f: F,
) -> Result<R>
where
    F: FnOnce() -> R,
{
    restore_terminal(terminal, keyboard_enhanced)?;

    let value = match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => value,
        // The terminal is already restored and main.rs's panic hook has
        // already run (hooks fire at the panic site, before catch_unwind
        // returns), so rebuilding here would re-enter raw mode and the
        // alternate screen with nothing left on the unwind path to undo it —
        // gui/mod.rs's restore_terminal is a statement, not a Drop guard, and
        // is skipped while unwinding.
        Err(payload) => panic::resume_unwind(payload),
    };

    enter_terminal_modes(Some(keyboard_enhanced))?;
    terminal.clear()?;
    Ok(value)
}

/// Run a terminal editor to completion with inherited stdio, so it owns the TTY.
fn run_editor_blocking(cmd: &EditorCmd, req: &EditRequest) -> Result<()> {
    let cmd_str = editor::expand(&cmd.template, &req.path, req.line, req.column);
    crate::os::cmd::log_command(&cmd_str);
    let status = Command::new("sh").args(["-c", &cmd_str]).status()?;
    if !status.success() {
        match status.code() {
            // 127 is `sh`'s "command not found". Name it, because a mistyped
            // os.edit or a missing binary is the likeliest cause.
            Some(127) => bail!("editor not found (status 127): {cmd_str}"),
            Some(code) => bail!("editor exited with status {code}: {cmd_str}"),
            None => bail!("editor was killed by a signal: {cmd_str}"),
        }
    }
    Ok(())
}

/// Launch a GUI editor without touching the terminal.
fn run_editor_detached(cmd: &EditorCmd, req: &EditRequest) -> Result<()> {
    let cmd_str = editor::expand(&cmd.template, &req.path, req.line, req.column);
    crate::os::cmd::log_command(&cmd_str);
    Command::new("sh").args(["-c", &cmd_str]).spawn()?;
    Ok(())
}

/// Last resort when no editor could be resolved: the platform's file opener.
fn open_with_default_program(os: &OsConfig, path: &str) -> Result<()> {
    if os.open.is_empty() {
        bail!(
            "No editor configured. Set `os.edit` or `os.editPreset` in your \
             config.yml, or set $EDITOR."
        );
    }
    OsConfig::run_template(&os.open, path)
}

/// Why an edit action failed. The distinction matters: an editor that exits
/// non-zero deserves a popup, while a terminal we could not rebuild is fatal
/// because nothing can be drawn afterwards.
pub enum EditError {
    Editor(anyhow::Error),
    Terminal(anyhow::Error),
}

/// Execute an edit request, suspending the TUI only when the editor needs it.
pub fn run_edit_request(
    terminal: &mut Term,
    keyboard_enhanced: bool,
    os: &OsConfig,
    req: EditRequest,
) -> Result<(), EditError> {
    match editor::resolve(os, req.line) {
        Some(cmd) if cmd.suspend => {
            let editor_outcome = run_with_terminal_suspended(terminal, keyboard_enhanced, || {
                run_editor_blocking(&cmd, &req)
            })
            .map_err(EditError::Terminal)?;
            editor_outcome.map_err(EditError::Editor)
        }
        Some(cmd) => run_editor_detached(&cmd, &req).map_err(EditError::Editor),
        None => open_with_default_program(os, &req.path).map_err(EditError::Editor),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> EditRequest {
        EditRequest::at("/tmp/lazygitrs-test-file".to_string(), None)
    }

    #[test]
    fn edit_request_at_defaults_the_column_to_one() {
        let r = EditRequest::at("/tmp/a.rs".to_string(), Some(3));
        assert_eq!(r.column, 1);
        assert_eq!(r.line, Some(3));
        assert_eq!(r.path, "/tmp/a.rs");
    }

    #[test]
    fn run_editor_blocking_succeeds_on_a_zero_exit() {
        let cmd = EditorCmd {
            template: "true {{filename}}".into(),
            suspend: true,
        };
        assert!(run_editor_blocking(&cmd, &req()).is_ok());
    }

    #[test]
    fn run_editor_blocking_reports_a_non_zero_exit() {
        let cmd = EditorCmd {
            template: "exit 3".into(),
            suspend: true,
        };
        let err = run_editor_blocking(&cmd, &req()).unwrap_err().to_string();
        // Not `contains('3')`: the expanded command line "exit 3" is echoed in
        // every arm, so a bare digit check passes even if the code is dropped.
        assert!(err.contains("status 3"), "exit code missing from: {err}");
        assert!(!err.contains("127"), "wrong arm taken: {err}");
    }

    #[test]
    fn run_editor_blocking_reports_a_missing_editor_as_127() {
        // The decisive case for surfacing non-zero exits: `sh -c` turns
        // "command not found" into a successful spawn with status 127, so
        // swallowing it would make a mistyped os.edit fail silently.
        let cmd = EditorCmd {
            template: "lazygitrs-no-such-editor-xyz {{filename}} 2>/dev/null".into(),
            suspend: true,
        };
        let err = run_editor_blocking(&cmd, &req()).unwrap_err().to_string();
        assert!(err.contains("127"), "expected status 127 in: {err}");
        assert!(err.contains("not found"), "127 arm not taken: {err}");
    }

    #[test]
    fn run_editor_blocking_quotes_a_path_containing_a_space() {
        // `test -f` on a path that does not exist fails, but a *mangled* argv
        // makes `test` fail differently — with 2 args it errors out. Using
        // `test -n` proves the path arrived as exactly one argument.
        let cmd = EditorCmd {
            template: "test -n {{filename}}".into(),
            suspend: true,
        };
        let request = EditRequest::at("/my repo/a b.rs".to_string(), None);
        assert!(run_editor_blocking(&cmd, &request).is_ok());
    }

    #[test]
    fn run_editor_detached_does_not_wait_for_the_child() {
        // Guards .spawn() against being changed to .status(): with .status()
        // this would block for 5 seconds and freeze the whole TUI for a user
        // with a GUI editor configured.
        let cmd = EditorCmd {
            template: "sleep 5".into(),
            suspend: false,
        };
        let started = std::time::Instant::now();
        assert!(run_editor_detached(&cmd, &req()).is_ok());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "run_editor_detached blocked for {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn open_with_default_program_errors_when_nothing_is_configured() {
        let os = OsConfig {
            open: String::new(),
            ..OsConfig::default()
        };
        let err = open_with_default_program(&os, "/tmp/a.rs")
            .unwrap_err()
            .to_string();
        assert!(err.contains("os.edit"), "unhelpful error: {err}");
        assert!(err.contains("os.editPreset"), "unhelpful error: {err}");
    }
}

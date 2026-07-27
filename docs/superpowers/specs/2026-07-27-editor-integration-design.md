# Editor integration: make `e` open files in a terminal editor

Date: 2026-07-27
Status: approved design, not yet implemented

## Problem

Pressing `e` in lazygitrs does not open the selected file in the user's editor. There are two
independent defects.

**1. No editor is configured, and the fallback is a GUI opener.**
`OsConfig::edit` defaults to `""` (`src/config/user_config.rs:253`). When it is empty, the edit
path falls through to `Platform::open_file` (`src/gui/controller/files.rs:538`), which runs
`xdg-open` on Linux (`src/os/platform.rs:14`). Nothing in the codebase consults `$VISUAL`,
`$EDITOR`, or `git config core.editor`.

**2. Setting `os.edit` would not fix it.**
`OsConfig::run_template` calls `.spawn()` and returns immediately
(`src/config/user_config.rs:277-279`). It neither waits for the child nor hands over the
terminal. lazygitrs still owns raw mode, the alternate screen, mouse capture, bracketed paste,
and keyboard-enhancement flags, so a terminal editor draws into a screen it does not control
while lazygitrs keeps consuming the keystrokes. There is no terminal-suspend helper anywhere in
the codebase.

`run_template` also splits the *substituted* command with `split_whitespace()`, so any repository
path containing a space is silently mangled into wrong argv, and templates cannot contain quoted
arguments.

### Why the obvious detection chain is wrong here

A naive chain of `$VISUAL` -> `$EDITOR` -> `$GIT_EDITOR` is actively harmful. `$GIT_EDITOR` is
git-specific and is commonly set to the no-op binary `true` to suppress git's interactive editor
prompts — it is `true` on the primary development machine for this change. Falling back to it
would launch `/usr/bin/true`, which exits 0 having done nothing, making `e` fail *silently*.
That is strictly worse than today's `xdg-open`.

`$GIT_EDITOR` is therefore excluded from the chain, and the literal values `true`, `false`, and
`:` are rejected wherever they appear in it.

## Goals

- `e` opens the selected file in the user's terminal editor, at the relevant line, with the
  editor owning the terminal; on exit, lazygitrs redraws cleanly and refreshes.
- Works out of the box from `$VISUAL`/`$EDITOR` with no configuration.
- GUI editors (VS Code, Zed, Sublime) launch detached without tearing down the TUI.
- Paths containing spaces and quotes work.
- Land the terminal-handoff helper that the tuicr review integration already depends on.

## Non-goals

- `lazygit-edit://` OSC-8 hyperlinks. The delta pager configured in this user's `config.yml`
  emits `lazygit-edit://{path}:{line}` links, and lazygitrs has no handler for that scheme
  (zero matches in `src/`). Making those links clickable is a separate feature.
- `os.editAtLineAndWait` and `os.openDirInEditor`. Both already exist in `OsConfig` and both are
  currently unread. They stay unread; nothing in this change depends on them.
- `$EDITOR`-based commit message editing (`src/gui/controller/files.rs:836`,
  `src/gui/mod.rs:5286`). The helper this change lands unblocks it, but it is out of scope.
- Binding `e` in the Commits, Branches, Stash, Tags, Remotes, or Worktrees panels.
- README changes. The README documents no configuration keys today, so there is nothing to
  extend. (If a config section is ever added, `just sync_readme` is required per `CLAUDE.md`.)

## Decisions

| Question | Decision |
| --- | --- |
| Which editor to run | lazygit-style `os.editPreset` table, with autodetection from the environment when unset |
| Which contexts | The 5 already-wired `e` sites, plus a new binding in the Commit Files panel |
| How templates become processes | `sh -c`, with `{{filename}}` shell-quoted |
| How a key handler reaches the terminal | Deferred request on `Gui`, drained in `main_loop` |
| Non-zero editor exit | Surfaced in an error popup, not swallowed |

## Architecture

### 1. `src/os/editor.rs` (new)

All editor resolution lives here as pure functions, so it is unit-testable without a TTY.

```rust
/// A resolved, ready-to-substitute editor invocation.
pub struct EditorCmd {
    pub template: String,
    /// True when the editor needs the real terminal (vim, nvim, nano, ...).
    pub suspend: bool,
}

pub struct Preset {
    pub name: &'static str,
    pub edit: &'static str,
    pub edit_at_line: &'static str,
    pub suspend: bool,
}

/// Look up a preset by its configured name or alias.
pub fn preset_by_name(name: &str) -> Option<&'static Preset>;

/// Match a raw `$EDITOR`-style value to a preset. Pure: takes the value, does not read the env.
pub fn preset_for_editor_string(value: &str) -> Option<&'static Preset>;

/// Full resolution. `line` is `None` when no line is known.
pub fn resolve(os: &OsConfig, line: Option<usize>) -> Option<EditorCmd>;

/// POSIX single-quote escaping.
pub fn shell_quote(s: &str) -> String;

/// Substitute `{{filename}}` (shell-quoted), `{{line}}`, `{{column}}`.
pub fn expand(template: &str, path: &str, line: Option<usize>, column: usize) -> String;
```

**Resolution precedence in `resolve`**, first match wins:

1. `os.edit_at_line` when `line.is_some()` and the field is non-empty; otherwise `os.edit` when
   non-empty. Explicit config always wins. `suspend` for this branch comes from
   `os.suspend_on_edit`, defaulting to `true` — an explicitly configured `os.edit` is far more
   likely to be a terminal editor than not.
2. `os.edit_preset` via `preset_by_name`.
3. Environment detection: `$VISUAL`, then `$EDITOR`, then `git config core.editor`. For each
   candidate: reject empty and reject `true` / `false` / `:`; take the first whitespace-separated
   token, take its file name without extension, and feed that to `preset_for_editor_string`.
4. Generic fallback when a candidate was found but matched no preset: template
   `"<raw candidate value> {{filename}}"` with `suspend: true`. Using the raw value rather than
   the extracted token preserves invocations like `EDITOR="nvim -u NONE"`. `--` is deliberately
   *not* inserted for unknown editors, since not every editor accepts it.
5. `None` when no candidate was found at all.

`os.suspend_on_edit`, when set, overrides the `suspend` value of whatever branch matched.

The environment- and git-derived part of step 3 is memoised in a `once_cell::sync::Lazy` so the
`git config core.editor` subprocess runs at most once per session. `once_cell` is already a
dependency (`Cargo.toml:23`).

### 2. Preset table

Modelled on lazygit's editor presets. `{{column}}` is only meaningful for editors that accept it.

| Name (aliases) | `edit` | `edit_at_line` | `suspend` |
| --- | --- | --- | --- |
| `vi` | `vi -- {{filename}}` | `vi +{{line}} -- {{filename}}` | true |
| `vim` | `vim -- {{filename}}` | `vim +{{line}} -- {{filename}}` | true |
| `nvim` | `nvim -- {{filename}}` | `nvim +{{line}} -- {{filename}}` | true |
| `lvim` | `lvim -- {{filename}}` | `lvim +{{line}} -- {{filename}}` | true |
| `nvim-remote` (`nvr`) | `nvr -- {{filename}}` | `nvr +{{line}} -- {{filename}}` | false |
| `helix` (`hx`) | `hx -- {{filename}}` | `hx -- {{filename}}:{{line}}` | true |
| `kakoune` (`kak`) | `kak -- {{filename}}` | `kak +{{line}} -- {{filename}}` | true |
| `nano` | `nano -- {{filename}}` | `nano +{{line}} -- {{filename}}` | true |
| `micro` | `micro {{filename}}` | `micro {{filename}}:{{line}}` | true |
| `emacs` | `emacs --no-window-system -- {{filename}}` | `emacs --no-window-system +{{line}} -- {{filename}}` | true |
| `vscode` (`code`) | `code --reuse-window -- {{filename}}` | `code --reuse-window --goto -- {{filename}}:{{line}}:{{column}}` | false |
| `vscodium` (`codium`) | `codium --reuse-window -- {{filename}}` | `codium --reuse-window --goto -- {{filename}}:{{line}}:{{column}}` | false |
| `sublime` (`subl`) | `subl -- {{filename}}` | `subl -- {{filename}}:{{line}}` | false |
| `zed` | `zed -- {{filename}}` | `zed -- {{filename}}:{{line}}:{{column}}` | false |
| `bbedit` | `bbedit {{filename}}` | `bbedit +{{line}} {{filename}}` | false |
| `xcode` (`xed`) | `xed -- {{filename}}` | `xed --line {{line}} -- {{filename}}` | false |
| `acme` | `acme {{filename}}` | `acme {{filename}}:{{line}}` | false |
| `notepadpp` (`notepad++`) | `notepad++ {{filename}}` | `notepad++ -n{{line}} {{filename}}` | false |

### 3. `OsConfig` additions (`src/config/user_config.rs:220`)

Two fields, using lazygit's exact YAML key names. `src/config/mod.rs:20` already falls back to
`~/.config/lazygit/config.yml`, so key compatibility means an existing lazygit config keeps
working unchanged.

```rust
/// Named editor preset, e.g. "nvim" or "vscode". Empty means autodetect.
#[serde(rename = "editPreset")]
pub edit_preset: String,          // default ""

/// Force terminal handover on or off, overriding the preset's own value.
#[serde(rename = "suspendOnEdit")]
pub suspend_on_edit: Option<bool>, // default None
```

### 4. `run_with_terminal_suspended` (`src/gui/mod.rs`)

Lives in `src/gui/interactive.rs` (see below), calling the now-`pub(super)` `setup_terminal` /
`restore_terminal` (`src/gui/mod.rs:7718-7774`). This is the
signature the tuicr review design already specified
(`docs/superpowers/specs/2026-07-11-tuicr-review-integration-design.md:118`); implementing it
here satisfies that dependency.

```rust
fn run_with_terminal_suspended<F, R>(
    terminal: &mut Term,
    keyboard_enhanced: bool,
    f: F,
) -> Result<R>
where
    F: FnOnce() -> Result<R>;
```

Behaviour:

1. `restore_terminal(terminal, keyboard_enhanced)` — leaves the alternate screen, disables raw
   mode and mouse capture, pops keyboard-enhancement flags, drains pending input.
2. Run `f()` inside `std::panic::catch_unwind` (with `AssertUnwindSafe`).
3. Unconditionally rebuild the terminal via the `setup_terminal` sequence, then `terminal.clear()`
   so ratatui's buffer diffing cannot leave stale cells from the editor's output.
4. If `f` panicked, `resume_unwind` **after** step 3, so the terminal is whole first.
5. Otherwise return `f`'s `Result` for the caller to turn into an error popup.

Step 3 must run on every exit path. If the rebuild itself fails, that error is returned; the
caller propagates it out of `main_loop`, and `run()` (`src/gui/mod.rs:658`) still executes its own
`restore_terminal`.

### 5. `src/gui/interactive.rs` (new) — deferred request

`src/gui/mod.rs` is already over 7,800 lines, so the new types and process-launching helpers go
in their own module rather than growing it further. `setup_terminal` and `restore_terminal` become
`pub(super)` so `run_with_terminal_suspended` can live here too. `src/gui/mod.rs` then gains only
the `Gui` field and the drain call.

This module holds `Interactive`, `EditRequest`, `run_with_terminal_suspended`,
`run_editor_blocking`, `run_editor_detached`, and `open_with_default_program`.

```rust
pub struct EditRequest {
    /// Absolute path to a file that has already been verified to exist.
    pub path: String,
    pub line: Option<usize>,
    /// 1-based; `1` when the call site has no column information.
    pub column: usize,
}

pub enum Interactive {
    Edit(EditRequest),
    // Review(..) — reserved for the tuicr <c-r> integration.
}
```

`Gui` gains `pub pending_interactive: Option<Interactive>`, defaulting to `None`
(`src/gui/mod.rs:284`, initialised near `:556`).

An enum rather than a boxed closure: the request is plain data, so a key handler is a pure
function from state to request and needs no terminal to test, and the terminal-handling code
stays in exactly one place.

Only one request can be pending. A second `e` before the drain would overwrite the first, which
cannot happen in practice because the drain runs in the same loop iteration as the keypress.

### 6. Drain point in `main_loop`

Inserted at `src/gui/mod.rs:1011` — after the event-handling block ends, before the
`should_quit` check at `:1012` and the `needs_refresh` block at `:1026`, so the post-edit refresh
happens in the same iteration.

```rust
if let Some(Interactive::Edit(req)) = self.pending_interactive.take() {
    let outcome = match editor::resolve(&self.config.user_config.os, req.line) {
        Some(cmd) if cmd.suspend => run_with_terminal_suspended(
            terminal,
            keyboard_enhanced,
            || run_editor_blocking(&cmd, &req),
        ),
        Some(cmd) => run_editor_detached(&cmd, &req),
        None => open_with_default_program(&self.config.user_config.os, &req.path),
    };
    if let Err(err) = outcome {
        self.show_error("Editor failed", err);
    }
    self.needs_refresh = true;
}
```

`main_loop` already receives `terminal: &mut Term` (`src/gui/mod.rs:662`), but
`keyboard_enhanced` is currently consumed only by `run()` (`:649`). It must be threaded into
`main_loop` as a second parameter.

- `run_editor_blocking`: `Command::new("sh").args(["-c", &expanded])` with **inherited** stdio and
  `.status()`. `sh` execs the editor, which owns the real TTY. Blocks until exit.
- `run_editor_detached`: `.spawn()` and return — today's behaviour, correct for GUI editors.
- `open_with_default_program`: `OsConfig::run_template(&os.open, path)`, preserving today's
  `xdg-open` fallback.

`needs_refresh` is set unconditionally: the file may have been modified on disk, and `refresh()`
sets `needs_diff_refresh` at `:1031`, so the open diff reloads too.

### 7. Call sites

Each becomes: resolve an absolute path, resolve a line/column, verify the file exists, set
`pending_interactive`. The duplicated `edit_at_line`-else-`edit` template picking currently
repeated at every site is deleted — `editor::resolve` owns it.

| Site | Context | Line source |
| --- | --- | --- |
| `src/gui/controller/files.rs:492` | Files panel | first changed hunk when the diff for that file is loaded (`hunk_starts.first()`) |
| `src/gui/mod.rs:2900` | diff panel focused | current diff line |
| `src/gui/mod.rs:2550` | diff text selection | selection line, plus column from the panel layout |
| `src/gui/controller/diff_mode.rs:471` | diff mode | current diff line |
| `src/gui/mod.rs:5045` | conflict file open | none |
| `src/gui/controller/commit_files.rs` | Commit Files panel (**new**) | none |

`OsConfig::run_template` / `run_template_at_line` keep their existing **non-blocking `.spawn()`
semantics** — their only remaining callers are `os.open` / "open in default program"
(`src/gui/controller/files.rs:544`, `src/gui/mod.rs:2962`), which must not block the TUI. They are
rewritten to build their command line via `editor::expand` and run it through `sh -c`, which fixes
the `split_whitespace()` mangling of paths containing spaces for those callers too.

### 8. New Commit Files binding

`commit_files::handle_key` gains a `keybindings.universal.edit` branch that opens the
**working-tree** version of the selected file. A historical blob cannot be edited in place; if
the path no longer exists in the working tree, show an error instead of creating a new file.

`commit_files::handle_key` serves three contexts (`src/gui/mod.rs:2504`): `CommitFiles`,
`StashFiles`, and `BranchCommitFiles`. One binding covers all three, and all three help sections
must gain the entry — `src/gui/mod.rs:4574` (`BranchCommits | BranchCommitFiles`), `:4688`
(`CommitFiles`), and `:4767` (`StashFiles`) — matching the existing `"Open in editor"` entry at
`:4446`. The repository's `audit-help` skill should be run afterwards to confirm the `?` dialog
matches the handlers.

## Data flow

```
key 'e'
  -> controller builds EditRequest { path (absolute), line, column }
  -> gui.pending_interactive = Some(Interactive::Edit(req)); return Ok(())

main_loop @ src/gui/mod.rs:1011   (owns &mut Term)
  -> pending_interactive.take()
  -> editor::resolve(&os, req.line)
       Some(cmd), suspend  -> run_with_terminal_suspended(term, kbd, || run_editor_blocking(..))
                                restore terminal
                                sh -c "nvim +42 -- '/path/to/file.rs'"   (inherited stdio, .status())
                                rebuild terminal, clear()
       Some(cmd), detached -> run_editor_detached(..)        // GUI editor, terminal untouched
       None                -> run_template(&os.open, path)   // xdg-open
  -> needs_refresh = true                                    // consumed at :1026 this iteration
  -> Err -> show_error("Editor failed", err)
```

On the primary development machine (`$VISUAL=nvim`, `$EDITOR=nvim`, `$GIT_EDITOR=true`,
`core.editor` unset) resolution reaches step 3, matches the `nvim` preset, and produces
`sh -c "nvim +42 -- '/path/to/file.rs'"` with `suspend: true`.

## Error handling

1. **`sh -c` fails to spawn** — returned as `Err`, shown as `"Editor failed"`.
2. **Editor exits non-zero** — returned as `Err` naming the exit code and the expanded command.
   This is deliberate: `sh -c` converts *command not found* into a successful spawn with exit
   status 127, so swallowing non-zero statuses would make a mistyped `os.edit` or a missing
   editor binary fail completely silently. The cost is an error popup after a deliberate `:cq`
   in vim, which is rare and honest.
3. **Panic inside the closure** — caught, terminal restored, then re-raised. A panic must never
   leave the terminal in raw mode.
4. **Terminal rebuild fails after the editor exits** — propagated out of `main_loop` so `run()`'s
   `restore_terminal` still runs. The application cannot continue without a usable terminal.
5. **File missing** — the call site does not set `pending_interactive`; it shows
   `"Cannot edit"` with the path. Relevant to the Commit Files binding, where the file may have
   been deleted after the commit being viewed.
6. **Nothing resolvable and `os.open` empty** — error popup naming `os.edit` and `os.editPreset`
   so the fix is discoverable.

Rendering is safe by construction: only `main_loop` draws, and it is blocked inside the closure
for the whole editor session, so no background thread can interleave output. The auto-refresh
timer (`src/gui/mod.rs:1017`) may elapse during the edit and fire on return, which is harmless
because the drain sets `needs_refresh` anyway.

## Testing

Inline `#[cfg(test)] mod tests`, matching the existing convention (19 files in `src/` use it).
No test in this repository constructs a `Gui`, so the split between automated and manual
verification is explicit.

**Unit tests — `src/os/editor.rs`:**

- `shell_quote`: plain path; path with spaces; path with an embedded single quote
  (`don't.rs` -> `'don'\''t.rs'`); non-ASCII path; empty string.
- `expand`: each placeholder individually; `{{filename}}` appearing twice; template with no
  placeholder; `line: None` leaves no stray `{{line}}` in the output.
- `preset_by_name`: canonical name; alias (`hx`, `code`, `nvr`); unknown name -> `None`.
- `preset_for_editor_string`: `nvim`; absolute path `/usr/local/bin/hx`; value with arguments
  `nvim -u NONE`; `code`; and `true`, `false`, `:`, `""` all -> `None`. The last group is the
  regression test for the `$GIT_EDITOR=true` trap described above.
- `resolve` precedence: explicit `edit_at_line` beats `edit_preset`; `edit_preset` beats a
  detected editor; `suspend_on_edit: Some(false)` flips a terminal preset to detached;
  `suspend_on_edit: Some(true)` flips a GUI preset to suspended; `line: None` selects `edit`
  rather than `edit_at_line` even when both are configured.

**Unit tests — `src/config/user_config.rs`:**

- YAML with `os: { editPreset: nvim, suspendOnEdit: false }` deserialises into the new fields.
- YAML with no `os:` block yields `edit_preset == ""` and `suspend_on_edit == None`.

**Manual verification** (requires a TTY, cannot be automated here):

- With `$EDITOR=nvim` and no `os:` config, press `e` in each of the six contexts. Confirm nvim
  takes over the terminal, opens at the expected line, and that on `:q` lazygitrs redraws with
  no stale cells and refreshes the file list.
- Edit and save a file, quit nvim, confirm the Files panel and the open diff both reflect the
  change without a manual refresh.
- Set `os.editPreset: vscode` and confirm `e` launches detached with the TUI never torn down.
- Set `os.edit: 'definitely-not-a-real-editor {{filename}}'` and confirm the exit-127 error
  popup appears rather than nothing happening.
- In the Commit Files panel, select a file that was deleted after the commit being viewed and
  confirm the `"Cannot edit"` error rather than an empty new file.
- Run the `audit-help` skill to confirm the `?` dialog matches the handlers.

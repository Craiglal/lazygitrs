# tuicr Review Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a lazygitrs user press `Ctrl+R` in the Files / Commits / CommitFiles panel to open tuicr's review UI, in-process, on that exact target, and return to lazygitrs on quit.

**Architecture:** Fork tuicr and expose its main-loop as a library function `run_review(CliArgs)`. lazygitrs depends on that fork behind an opt-in `review` Cargo feature. A `Ctrl+R` handler records a feature-agnostic `ReviewTarget` on the `Gui`; the main loop hands the terminal to tuicr via a generic suspend/restore helper, then rebuilds lazygitrs's terminal. The two ratatui/crossterm versions coexist (compile-time only) because they drive the terminal sequentially, never at once.

**Tech Stack:** Rust (edition 2024), ratatui 0.29 + crossterm 0.28 (lazygitrs, unchanged), ratatui 0.30 + crossterm 0.29 + git2 + syntect (tuicr fork, pulled only under `--features review`).

## Global Constraints

- lazygitrs stays on **ratatui 0.29 / crossterm 0.28** — NO TUI-stack upgrade. (Copied from spec: version coexistence is intentional.)
- The `review` Cargo feature is **off by default**. A default `cargo build` must not pull in tuicr, git2, or syntect, and must behave exactly as today.
- With the feature off, `Ctrl+R` shows an info popup ("built without review support"); the `Gui` struct and all non-tuicr code compile identically regardless of feature.
- Trigger key is `<c-r>` (Ctrl+R), configurable via `keybinding.universal.review`. Confirmed free in the current keymap.
- Single commit is passed to tuicr as the **bare hash** in `CliArgs.revisions` (tuicr diffs it against its first parent). No `^!`, no explicit range.
- When embedding, always set `CliArgs.output_to_stdout = false` and `CliArgs.no_update_check = true`.
- The tuicr fork change must be **behavior-preserving** for tuicr's own binary and stay upstreamable.
- git2 must be built with its `vendored` feature so the `review` build needs no system libgit2.
- v1 scope only: working-tree, single-file, single-commit. No PR mode, no ranges, no in-panel review badges.

---

## Repository A — tuicr fork

> Work in a clone of your fork of `github.com/agavra/tuicr` on a branch `embed-run`.
> All line numbers below refer to tuicr **v0.19.0** (`src/main.rs` is 818 lines); if your
> fork is a different revision, locate the equivalent items by name, not by number.

### Task A1: Make `CliArgs` constructible by dependents

**Files:**
- Modify: `src/cli.rs` (the `pub struct CliArgs` at line ~14)

**Interfaces:**
- Produces: `CliArgs: Default` — a public struct dependents build with struct-update syntax.

- [ ] **Step 1: Add `Default` to the derive on `CliArgs`**

In `src/cli.rs`, find:

```rust
pub struct CliArgs {
    pub theme: Option<String>,
    pub appearance: Option<AppearanceArg>,
    pub output_to_stdout: bool,
    pub no_update_check: bool,
    pub revisions: Option<String>,
    pub working_tree: bool,
    pub path_filter: Option<String>,
    pub file_path: Option<String>,
    pub all_files: bool,
    pub pr_target: Option<String>,
    pub repo_url: Option<String>,
    pub review_command: Option<ReviewCommand>,
}
```

Add `Default` to its `#[derive(...)]` line (all fields are `bool` or `Option<_>`, so this
compiles without touching `AppearanceArg`/`ReviewCommand`). If `CliArgs` has no derive
attribute, add one:

```rust
#[derive(Debug, Default)]
pub struct CliArgs { /* unchanged */ }
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build`
Expected: builds clean.

- [ ] **Step 3: Add a test proving the default is inert**

Add to `src/cli.rs` (or its test module):

```rust
#[cfg(test)]
mod embed_tests {
    use super::*;

    #[test]
    fn cliargs_default_is_empty() {
        let a = CliArgs::default();
        assert!(!a.working_tree);
        assert!(a.revisions.is_none());
        assert!(a.path_filter.is_none());
        assert!(!a.output_to_stdout);
        assert!(!a.no_update_check);
    }
}
```

- [ ] **Step 4: Run the test**

Run: `cargo test cliargs_default_is_empty`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/cli.rs
git commit -m "feat(cli): derive Default on CliArgs for embedding"
```

### Task A2: Relocate the review loop into a library `run_review`

**Files:**
- Create: `src/run.rs`
- Modify: `src/lib.rs`, `src/main.rs`

**Interfaces:**
- Consumes: `CliArgs` (Task A1), and existing lib items (`App`, `ui`, `handler`, `config`, `theme`, …).
- Produces: `pub fn run_review(cli_args: CliArgs) -> anyhow::Result<()>` re-exported from the crate root.

This is a mechanical **move**, not a rewrite. The body already exists inside `fn main()`.

- [ ] **Step 1: Create `src/run.rs` and move the loop + its local helpers into it**

Cut from `src/main.rs` and paste into `src/run.rs`:
- the constants `CTRL_C_EXIT_TIMEOUT` (line ~29) and `MIN_WIDTH_FOR_FILE_LIST` (line ~31);
- the helper fns `dispatch_action` (~712), `handle_comment_vim_key` (~735), `run_editor_from_tui` (~810);
- **the entire body of `fn main()` starting at the keyboard-enhancement probe** (the line
  `let keyboard_enhancement_supported = ...`, ~line 55) **through the final `Ok(())`** of the
  TUI run (~line 711, immediately before `fn dispatch_action`).

Wrap the moved body in the new public function, taking `cli_args` by value (it was a local
`let mut cli_args` in `main`; keep it `mut`):

```rust
// src/run.rs
use crate::*; // mirror the `use` items main.rs relied on; add explicit `use`s the compiler asks for.

pub const CTRL_C_EXIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
pub const MIN_WIDTH_FOR_FILE_LIST: u16 = 100;

pub fn run_review(mut cli_args: CliArgs) -> anyhow::Result<()> {
    // <-- moved body: keyboard-enhancement probe … main loop … cleanup … Ok(())
}

fn dispatch_action(app: &mut App, action: Action) { /* moved verbatim */ }
fn handle_comment_vim_key(app: &mut App, key: crossterm::event::KeyEvent) -> bool { /* moved verbatim */ }
fn run_editor_from_tui<W: std::io::Write>(/* unchanged signature */) { /* moved verbatim */ }
```

Do not change logic. Fix only visibility/imports: any item the moved code referenced that
lived in `main.rs` moved with it; items from other modules (`parse_cli_args`,
`resolve_theme_with_config`, `supports_keyboard_enhancement`, `App`, `ui::render`, `Action`,
etc.) are imported via `use`. Resolve each unresolved-name error by adding the `use` the
compiler names.

- [ ] **Step 2: Register the module and re-export the entry point**

In `src/lib.rs`, add alongside the other `pub mod` lines:

```rust
pub mod run;
pub use run::run_review;
```

- [ ] **Step 3: Reduce `fn main()` to a shim that calls `run_review`**

`src/main.rs` should now contain only startup that must stay in the binary. Replace the old
body after the `review_command` early-return with a call:

```rust
fn main() -> anyhow::Result<()> {
    profile::init_from_env();

    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        tuicr::terminal_state::restore_stdio_best_effort();
        original_hook(panic_info);
    }));

    let mut cli_args = profile::time("startup.parse_cli_args", parse_cli_args);
    if let Some(review_command) = cli_args.review_command.take() {
        if let Err(err) = tuicr::review_cli::run(review_command) {
            eprintln!("Error: {err}");
            std::process::exit(1);
        }
        return Ok(());
    }

    tuicr::run_review(cli_args)
}
```

Keep `parse_cli_args` where it is (in the binary) unless the compiler reports it is needed by
`run.rs`; if so, move it into the library too and import it.

- [ ] **Step 4: Verify the whole crate builds (lib + bin)**

Run: `cargo build`
Expected: builds clean. Fix any leftover import/visibility errors by name.

- [ ] **Step 5: Verify tuicr's own test suite still passes**

Run: `cargo test`
Expected: same pass/fail set as before the move (no new failures).

- [ ] **Step 6: Manually confirm the binary behaves identically**

Run in a scratch git repo with an uncommitted change:
```bash
cargo run -- -w
```
Expected: tuicr opens the working-tree review exactly as before; `q` exits cleanly and the
terminal is restored. Repeat once with a commit hash: `cargo run -- -r <hash>`.

- [ ] **Step 7: Commit and push the branch**

```bash
git add src/run.rs src/lib.rs src/main.rs
git commit -m "refactor: expose review loop as library run_review(CliArgs)"
git push -u origin embed-run
```

Record the fork URL + branch — lazygitrs Task B1 needs them.

---

## Repository B — lazygitrs

> Prerequisite: Task A2 pushed; you have the fork's git URL and `embed-run` branch name.
> Substitute them for `<FORK_URL>` below.

### Task B1: Add the optional tuicr dependency and `review` feature

**Files:**
- Modify: `Cargo.toml`

**Interfaces:**
- Produces: Cargo feature `review` enabling an optional `tuicr` dependency. `cfg(feature = "review")` is now usable in code.

- [ ] **Step 1: Add the optional dependency and feature**

In `Cargo.toml`, under `[dependencies]` add:

```toml
tuicr = { git = "<FORK_URL>", branch = "embed-run", optional = true, default-features = false }
```

Add a `[features]` section (the file has none today):

```toml
[features]
default = []
review = ["dep:tuicr"]
```

> If tuicr does not build git2 vendored by default, also enable it here, e.g.
> `review = ["dep:tuicr", "tuicr/vendored-git2"]`, or add `git2 = { version = "0.20",
> features = ["vendored"] }` as an optional dep. Confirm against the fork's feature names
> during this step; the goal is: the `review` build needs no system libgit2.

- [ ] **Step 2: Verify the default build is unchanged**

Run: `cargo build`
Expected: builds clean, does NOT download/compile tuicr, git2, or syntect (watch the compile
list; none should appear).

- [ ] **Step 3: Verify the feature build resolves tuicr**

Run: `cargo build --features review`
Expected: fetches and compiles the fork (a second ratatui 0.30 / crossterm 0.29 appear in the
build — expected). Builds clean.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "build: add optional tuicr dep behind off-by-default 'review' feature"
```

### Task B2: Feature-agnostic scaffolding — keybinding, `ReviewTarget`, Gui fields

**Files:**
- Modify: `src/config/keybindings.rs` (`UniversalKeybinding` struct + its `Default`, ~line 100 / ~129)
- Create: `src/gui/review_target.rs`
- Modify: `src/gui/mod.rs` (module decl; `Gui` struct fields ~line 296; `Gui` initializer ~line 556; `run()` ~line 649)

**Interfaces:**
- Produces:
  - `keybinding.universal.review: String` (default `"<c-r>"`).
  - `enum ReviewTarget { WorkingTree { path: Option<String> }, Commit { hash: String, path: Option<String> } }` (public in the gui module).
  - `Gui.pending_review: Option<ReviewTarget>`, `Gui.keyboard_enhanced: bool`.

- [ ] **Step 1: Add the `review` keybinding field**

In `src/config/keybindings.rs`, in `pub struct UniversalKeybinding` (near `pub refresh: String,`
at ~line 100) add:

```rust
    pub review: String,
```

In `impl Default for UniversalKeybinding` (~line 129), near `refresh: "R".into(),` add:

```rust
            review: "<c-r>".into(),
```

- [ ] **Step 2: Define `ReviewTarget`**

Create `src/gui/review_target.rs`:

```rust
//! Feature-agnostic description of what tuicr should review. Compiled in all
//! builds; converted to `tuicr::CliArgs` only under `feature = "review"`.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewTarget {
    /// Review the working tree; `path` optionally restricts to one file.
    WorkingTree { path: Option<String> },
    /// Review a single commit (diffed against its first parent by tuicr);
    /// `path` optionally restricts to one file.
    Commit { hash: String, path: Option<String> },
}
```

Register it in `src/gui/mod.rs` with the other `mod` declarations:

```rust
mod review_target;
pub use review_target::ReviewTarget;
```

- [ ] **Step 3: Add the two `Gui` fields**

In `src/gui/mod.rs`, in the `Gui` struct (near `pub needs_refresh: bool,` ~line 296) add:

```rust
    pub pending_review: Option<ReviewTarget>,
    pub keyboard_enhanced: bool,
```

In the `Gui` initializer (near `needs_refresh: false,` ~line 556) add:

```rust
            pending_review: None,
            keyboard_enhanced: false,
```

- [ ] **Step 4: Record `keyboard_enhanced` in `run()`**

In `src/gui/mod.rs` `run()` (~line 649), after `setup_terminal()`:

```rust
        let (mut terminal, keyboard_enhanced) = setup_terminal()?;
        self.keyboard_enhanced = keyboard_enhanced;
```

- [ ] **Step 5: Verify both build configs compile**

Run: `cargo build && cargo build --features review`
Expected: both clean. (Nothing consumes the new items yet — that's fine.)

- [ ] **Step 6: Commit**

```bash
git add src/config/keybindings.rs src/gui/review_target.rs src/gui/mod.rs
git commit -m "feat(gui): add review keybinding, ReviewTarget, and pending-review scaffolding"
```

### Task B3: Map the active context to a `ReviewTarget`

**Files:**
- Modify: `src/gui/mod.rs` (new method + test module)

**Interfaces:**
- Consumes: `self.context_mgr.active()` (`ContextId`), `self.context_mgr.selected_active()`,
  `self.selected_file_index()`, `self.model` (`commits`, `files`, `commit_files`),
  `self.commit_files_hash`, `ReviewTarget`.
- Produces: `fn build_review_target(&self) -> Option<ReviewTarget>`.

- [ ] **Step 1: Write failing tests for the mapper**

Add a test module in `src/gui/mod.rs`. Build a `Gui` via the crate's existing test helper if
one exists; otherwise assert on a small pure sub-function. To keep the logic unit-testable
without constructing a full `Gui`, factor the decision into a **pure free function** and test
that:

```rust
#[cfg(test)]
mod review_target_tests {
    use super::*;
    use crate::gui::context::ContextId;

    // Pure core: (context, selected commit hash, selected file path) -> target.
    #[test]
    fn files_context_reviews_working_tree() {
        assert_eq!(
            review_target_for(ContextId::Files, None, Some("src/a.rs".into())),
            Some(ReviewTarget::WorkingTree { path: Some("src/a.rs".into()) }),
        );
    }

    #[test]
    fn files_context_no_selection_reviews_whole_tree() {
        assert_eq!(
            review_target_for(ContextId::Files, None, None),
            Some(ReviewTarget::WorkingTree { path: None }),
        );
    }

    #[test]
    fn commits_context_reviews_bare_hash() {
        assert_eq!(
            review_target_for(ContextId::Commits, Some("abc123".into()), None),
            Some(ReviewTarget::Commit { hash: "abc123".into(), path: None }),
        );
    }

    #[test]
    fn commit_files_context_restricts_to_path() {
        assert_eq!(
            review_target_for(ContextId::CommitFiles, Some("abc123".into()), Some("src/a.rs".into())),
            Some(ReviewTarget::Commit { hash: "abc123".into(), path: Some("src/a.rs".into()) }),
        );
    }

    #[test]
    fn unsupported_context_returns_none() {
        assert_eq!(review_target_for(ContextId::Branches, None, None), None);
    }
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test review_target_tests`
Expected: FAIL — `review_target_for` not found.

- [ ] **Step 3: Implement the pure core plus the `Gui` method that feeds it**

Add to `src/gui/mod.rs`:

```rust
/// Pure mapping from the active context and current selection to a review
/// target. `commit_hash` is the selected commit's hash (Commits / BranchCommits
/// / CommitFiles); `file_path` is the repo-relative path of the selected file,
/// if any.
fn review_target_for(
    context: crate::gui::context::ContextId,
    commit_hash: Option<String>,
    file_path: Option<String>,
) -> Option<ReviewTarget> {
    use crate::gui::context::ContextId;
    match context {
        ContextId::Files => Some(ReviewTarget::WorkingTree { path: file_path }),
        ContextId::Commits | ContextId::BranchCommits => {
            commit_hash.map(|hash| ReviewTarget::Commit { hash, path: None })
        }
        ContextId::CommitFiles => {
            commit_hash.map(|hash| ReviewTarget::Commit { hash, path: file_path })
        }
        _ => None,
    }
}

impl Gui {
    /// Resolve what tuicr should review, from the live selection. Returns
    /// `None` when the active panel has no reviewable target.
    fn build_review_target(&self) -> Option<ReviewTarget> {
        let context = self.context_mgr.active();
        let model = self.model.lock().unwrap();

        let commit_hash = match context {
            crate::gui::context::ContextId::Commits
            | crate::gui::context::ContextId::BranchCommits => model
                .commits
                .get(self.context_mgr.selected_active())
                .map(|c| c.hash.clone()),
            crate::gui::context::ContextId::CommitFiles => {
                Some(self.commit_files_hash.clone()).filter(|h| !h.is_empty())
            }
            _ => None,
        };

        let file_path = match context {
            crate::gui::context::ContextId::Files => self
                .selected_file_index()
                .and_then(|i| model.files.get(i))
                .map(|f| f.current_path().to_string()),
            crate::gui::context::ContextId::CommitFiles => self
                .selected_file_index()
                .and_then(|i| model.commit_files.get(i))
                .map(|f| f.current_path().to_string()),
            _ => None,
        };

        drop(model);
        review_target_for(context, commit_hash, file_path)
    }
}
```

> Adjust `commits`/`commit_files` field access and `current_path()`/`hash` to the exact
> accessor names in `src/model` and `src/gui/mod.rs` (see the existing `resolve_template` in
> `src/gui/controller/custom_commands.rs` for the established patterns). The pure
> `review_target_for` is what the tests pin; `build_review_target` is thin glue over it.

- [ ] **Step 4: Run the tests**

Run: `cargo test review_target_tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/gui/mod.rs
git commit -m "feat(gui): map active context/selection to a ReviewTarget"
```

### Task B4: Generic terminal suspend/restore helper

**Files:**
- Modify: `src/gui/mod.rs` (new free fn near `setup_terminal`/`restore_terminal`, ~line 7718)

**Interfaces:**
- Consumes: `setup_terminal()`, `restore_terminal(&mut Term, bool)`, `Term`.
- Produces: `fn run_with_terminal_suspended<R>(terminal: &mut Term, keyboard_enhanced: bool, f: impl FnOnce() -> Result<R>) -> Result<R>`.

- [ ] **Step 1: Implement the helper**

Add near `restore_terminal` in `src/gui/mod.rs`:

```rust
/// Hand the terminal to an interactive callback (an external/embedded TUI),
/// guaranteeing lazygitrs's terminal is torn down first and rebuilt afterward
/// on every exit path — including a panic inside `f`. The rebuilt `Term`
/// replaces `*terminal` and the screen is cleared so no foreign output remains.
fn run_with_terminal_suspended<R>(
    terminal: &mut Term,
    keyboard_enhanced: bool,
    f: impl FnOnce() -> Result<R>,
) -> Result<R> {
    use std::panic::{catch_unwind, AssertUnwindSafe};

    restore_terminal(terminal, keyboard_enhanced)?;

    let outcome = catch_unwind(AssertUnwindSafe(f));

    // Always rebuild lazygitrs's terminal before propagating anything.
    let (new_terminal, kbd) = setup_terminal()?;
    *terminal = new_terminal;
    let _ = kbd; // caller updates Gui.keyboard_enhanced (see Task B5)
    let _ = terminal.clear();

    match outcome {
        Ok(result) => result,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}
```

> `setup_terminal()` returns `(Term, bool)`; the fresh `keyboard_enhanced` is re-read here.
> Task B5's caller writes it back to `self.keyboard_enhanced` after calling this helper.

- [ ] **Step 2: Verify both build configs compile**

Run: `cargo build && cargo build --features review`
Expected: both clean. (Unused-function warning is acceptable until Task B5 wires it.)

- [ ] **Step 3: Commit**

```bash
git add src/gui/mod.rs
git commit -m "feat(gui): add run_with_terminal_suspended handoff helper"
```

### Task B5: Wire `Ctrl+R` → launch tuicr (feature-gated) → refresh

**Files:**
- Modify: `src/gui/mod.rs` (`handle_key` ~line 2099; `main_loop` ~line 662; new `launch_review` method)

**Interfaces:**
- Consumes: `build_review_target` (B3), `run_with_terminal_suspended` (B4),
  `self.pending_review`/`self.keyboard_enhanced` (B2), `keybinding.universal.review` (B2),
  `tuicr::run_review` + `tuicr::CliArgs` (fork).
- Produces: end-to-end review launch.

- [ ] **Step 1: Set `pending_review` on `<c-r>` in `handle_key`**

In `src/gui/mod.rs` `handle_key` (~line 2099), before the `self.handle_context_key(key)?;`
call (alongside the other universal keys), add:

```rust
        if matches_key(key, &self.config.user_config.keybinding.universal.review) {
            self.pending_review = self.build_review_target();
            return Ok(());
        }
```

- [ ] **Step 2: Act on `pending_review` in `main_loop`**

In `main_loop`, immediately after the key-dispatch site (the block around line 949 that calls
`self.handle_key(key)`), add a check that runs once control returns to the loop level (where
`terminal` is owned):

```rust
            if let Some(target) = self.pending_review.take() {
                self.launch_review(terminal, target);
            }
```

- [ ] **Step 3: Implement `launch_review` — real under the feature, stub without it**

Add two `cfg`-gated methods to `impl Gui` in `src/gui/mod.rs`:

```rust
#[cfg(feature = "review")]
impl Gui {
    /// Suspend lazygitrs, run tuicr's review on `target` in-process, then
    /// restore. tuicr discovers the repo from the process CWD, so set it to the
    /// repo path for the duration and restore it afterward.
    fn launch_review(&mut self, terminal: &mut Term, target: ReviewTarget) {
        let cli_args = review_target_to_cli_args(target);
        let repo_path = self.git.repo_path().to_path_buf();
        let kbd = self.keyboard_enhanced;

        let prev_cwd = std::env::current_dir().ok();
        let result = run_with_terminal_suspended(terminal, kbd, || {
            if std::env::set_current_dir(&repo_path).is_err() {
                anyhow::bail!("could not enter repo directory for review");
            }
            let r = tuicr::run_review(cli_args).map_err(|e| anyhow::anyhow!(e.to_string()));
            if let Some(cwd) = &prev_cwd {
                let _ = std::env::set_current_dir(cwd);
            }
            r
        });

        // Re-read keyboard enhancement after the terminal was rebuilt.
        self.keyboard_enhanced = crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);

        if let Err(e) = result {
            self.popup = PopupState::Message {
                title: "Review failed".to_string(),
                message: e.to_string(),
                kind: MessageKind::Error,
            };
        }
        self.needs_refresh = true;
    }
}

#[cfg(feature = "review")]
fn review_target_to_cli_args(target: ReviewTarget) -> tuicr::CliArgs {
    let mut args = tuicr::CliArgs { no_update_check: true, output_to_stdout: false, ..Default::default() };
    match target {
        ReviewTarget::WorkingTree { path } => {
            args.working_tree = true;
            args.path_filter = path;
        }
        ReviewTarget::Commit { hash, path } => {
            args.revisions = Some(hash);
            args.path_filter = path;
        }
    }
    args
}

#[cfg(not(feature = "review"))]
impl Gui {
    fn launch_review(&mut self, _terminal: &mut Term, _target: ReviewTarget) {
        self.popup = PopupState::Message {
            title: "Review unavailable".to_string(),
            message: "This build was compiled without the `review` feature.".to_string(),
            kind: MessageKind::Info,
        };
        self.needs_refresh = true;
    }
}
```

> Confirm the `PopupState::Message` / `MessageKind` variants against their definitions in
> `src/gui` (the `custom_commands` controller uses the same popup) and that
> `tuicr::CliArgs`'s exported field names match Task A1. Adjust the error `map_err` to
> tuicr's actual `run_review` error type.

- [ ] **Step 4: Verify both build configs compile**

Run: `cargo build && cargo build --features review`
Expected: both clean.

- [ ] **Step 5: Smoke test — feature OFF**

Run: `cargo run` in a git repo; press `Ctrl+R` in the Files panel.
Expected: info popup "compiled without the `review` feature"; lazygitrs keeps running.

- [ ] **Step 6: Smoke test — feature ON, each trigger**

Run: `cargo run --features review` in a scratch repo that has an uncommitted change and at
least one commit. Verify each:
- Files panel, no file selected → `Ctrl+R` opens tuicr on the whole working tree.
- Files panel, a file selected → `Ctrl+R` opens tuicr scoped to that file.
- Commits panel, a commit selected → `Ctrl+R` opens tuicr on that commit's diff.
- CommitFiles (drill into a commit), a file selected → `Ctrl+R` opens tuicr scoped to that file in that commit.
For each: quit tuicr (`q`) → you land back in the same lazygitrs panel, screen intact, no
corruption. Trigger from the Branches panel → info/no-op (unsupported), no crash.

- [ ] **Step 7: Commit**

```bash
git add src/gui/mod.rs
git commit -m "feat(gui): launch tuicr review on Ctrl+R via in-process sub-loop"
```

### Task B6: Document the feature

**Files:**
- Modify: `README.md` (then run `just sync_readme` per repo convention)

**Interfaces:** none (docs).

- [ ] **Step 1: Add a short "Code review (tuicr)" section**

Document: the `Ctrl+R` binding and which panels support it; that it requires building with
`--features review` (off by default) and why (pulls tuicr + git2 + syntect); and that it
uses your tuicr fork. Keep it to a compact subsection matching the README's existing tone.

- [ ] **Step 2: Sync generated README copies**

Run: `just sync_readme`
Expected: `npm/README.md` (and any other synced copies) updated. (Per `CLAUDE.md`, README
changes must go through `just sync_readme`.)

- [ ] **Step 3: Commit**

```bash
git add README.md npm/README.md
git commit -m "docs(readme): document Ctrl+R tuicr code review (review feature)"
```

---

## Self-Review Notes (author checklist, resolved)

- **Spec coverage:** run_review extraction → A1/A2; handoff helper → B4; context→options
  launcher → B3+B5; keybinding `<c-r>` → B2/B5; `review` feature off by default → B1;
  terminal safety / catch_unwind → B4; error popup → B5; cwd handling → B5; docs → B6.
  Out-of-scope items (PR, ranges, badges) intentionally have no task.
- **Placeholders:** `<FORK_URL>` is a required real value supplied at B1 (the user's fork),
  not a hand-wave; every code step shows concrete code. Instructions to "confirm accessor
  names / variant names against the source" point at named existing files and patterns rather
  than inventing APIs.
- **Type consistency:** `ReviewTarget` variants (`WorkingTree{path}`, `Commit{hash,path}`),
  `build_review_target`/`review_target_for`, `run_with_terminal_suspended`, and
  `review_target_to_cli_args` names/signatures are used identically across B2–B5.
- **Testing honesty:** B3 is genuinely unit-tested (pure `review_target_for`). A2, B1, B4,
  B5 are verified by build + `cargo test` (tuicr's suite) + manual TUI smoke, because a
  terminal event loop and cross-repo move are not meaningfully covered by unit tests; this is
  stated rather than papered over with performative tests.

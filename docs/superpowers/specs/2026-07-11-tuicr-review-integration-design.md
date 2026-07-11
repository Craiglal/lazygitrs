# Design: Embed tuicr as an in-process review sub-loop in lazygitrs

**Date:** 2026-07-11
**Status:** Approved for planning
**Topic:** Launch [tuicr](https://github.com/agavra/tuicr) code review from inside lazygitrs, in-process, sharing the same terminal sequentially.

## Goal

From inside lazygitrs, press a key in the Files, Commits, or CommitFiles panel and drop
straight into tuicr's GitHub-style review of exactly that target (working tree, a commit's
diff, or a single file). Quitting tuicr returns you to lazygitrs in the same panel with the
same selection. One binary, no separate `tuicr` install, and review comments persist in
tuicr's own store.

Non-goals for v1: surfacing tuicr review state inside lazygitrs panels (data-level
integration), PR review mode, and multi-commit ranges.

## Background / constraints discovered

- **tuicr is a real library** (`src/lib.rs` re-exports every module `pub`), version 0.19,
  edition 2024, on **ratatui 0.30 + crossterm 0.29**, using **git2** (libgit2) and **syntect**.
- **lazygitrs** is edition 2024 on **ratatui 0.29 + crossterm 0.28**, shells out to the `git`
  CLI, and highlights with tree-sitter.
- tuicr's UI is **step-driven**: `ui::render(frame, &mut app)` is a pure render function;
  `App::new(theme, comment_type_configs, output_to_stdout, AppStartupOptions)` is a public
  constructor; and `handler::handle_*_action(app, action)` covers input dispatch.
- **The event loop is NOT in the library.** tuicr's terminal setup + main loop live entirely
  inside the binary's `fn main()` (roughly `src/main.rs:33`..`~700`), breaking on
  `app.should_quit`. There is no callable `run()` in the library.
- Because lazygitrs fully tears its terminal down before tuicr runs and rebuilds it after,
  the two ratatui/crossterm versions **coexist safely**: the duplication is compile-time
  only; at runtime the two stacks drive the terminal sequentially, never concurrently. No
  lazygitrs-wide ratatui/crossterm upgrade is required for this approach.
- lazygitrs currently has **no** "suspend TUI → run interactive subprocess → restore TUI"
  mechanism. `commit_with_editor` (`src/gui/controller/files.rs`) explicitly defers it as
  "Phase 4 (subprocess management)". This design builds that mechanism.

`AppStartupOptions` (from `src/app/mod.rs` in tuicr) maps cleanly onto lazygitrs contexts:

```rust
pub struct AppStartupOptions<'a> {
    pub revisions: Option<&'a str>,       // commit / range -> "<hash>^!"
    pub working_tree: bool,               // Files panel
    pub path_filter: Option<&'a str>,     // single file
    pub file_path: Option<&'a str>,       // standalone-file annotate mode
    pub all_files: bool,
    pub git_backend_preference: GitBackendPreference,
    pub diff_whitespace_mode: DiffWhitespaceMode,
    pub pr_target: Option<&'a str>,       // (later) "tuicr pr N"
    pub repo_url_override: Option<ForgeRepository>,
}
```

## Decisions (locked)

1. **Embed flavor:** in-process sub-loop (tuicr owns the screen while active, returns to
   lazygitrs on quit). Not a woven panel; not spawning the binary.
2. **Runnable entry point:** approach **A — fork tuicr and extract a library `run_review`**
   (see below). Rejected approach B (reconstructing tuicr's ~600-line loop as glue in
   lazygitrs) as too fragile against tuicr's internal `Action`/handler churn.
3. **Keybinding:** `<c-r>` (Ctrl+R), configurable via lazygitrs keybindings.
4. **Build:** tuicr dependency gated behind an **opt-in Cargo feature `review`, off by
   default**, so lean builds avoid git2/syntect and the second ratatui/crossterm.

## Architecture

Three components.

### 1. tuicr fork — expose `pub fn run_review`

In a personal fork of tuicr (matching the existing lazygitrs "slopfork" workflow), relocate
the tail of `fn main()` — everything **after** the `review_command` subcommand early-return
(keyboard-enhancement probe, config load, theme resolve, update-check thread, backend/
whitespace resolution, `App::new`, terminal setup, and the main loop) — into:

```rust
// tuicr fork, in src/lib.rs (or a new src/run.rs re-exported from lib)
pub fn run_review(opts: RunOptions) -> anyhow::Result<()>;
```

`RunOptions` mirrors the review-relevant CLI fields already parsed by tuicr's `parse_cli_args`:
`revisions`, `working_tree`, `path_filter`, `file_path`, `pr_target`, `repo_url_override`,
`theme`, `appearance`, `output_to_stdout` (always `false` when embedded), `no_update_check`
(force `true` when embedded — no background update thread inside another app).

The refactor is **behavior-preserving**: tuicr's own `fn main()` becomes a thin shim
(profile init, panic hook, parse args, handle the `review` subcommand, else call
`run_review(opts)`), so the fork's normal binary behaves identically and the change is
upstreamable. tuicr sets up and tears down its **own** crossterm-0.29 terminal inside
`run_review`; lazygitrs's terminal is already suspended at that point.

Maintenance: pull upstream tuicr into the fork, re-apply the ~30-line extract patch (one
commit) per release.

**Panic-hook note:** `run_review` must save the caller's current panic hook and restore it
on exit (tuicr's `main` installs its own restore-stdio hook). lazygitrs's `catch_unwind`
in component 2 is the backstop regardless.

### 2. Terminal-handoff helper (lazygitrs `gui`)

A reusable helper that hands the terminal to an interactive callback and guarantees
restoration on every exit path:

```rust
fn run_with_terminal_suspended<F, R>(
    terminal: &mut Term,
    keyboard_enhanced: bool,
    f: F,
) -> Result<R>
where F: FnOnce() -> Result<R>;
```

Behavior:
1. `restore_terminal(terminal, keyboard_enhanced)` — leave alt screen, disable raw mode,
   pop keyboard-enhancement flags (reuses the existing `restore_terminal`).
2. Run `f()` inside `std::panic::catch_unwind` so a panic in the callback (or in tuicr)
   cannot leave the terminal wrecked.
3. Always re-run `setup_terminal()`-equivalent to rebuild lazygitrs's terminal, then force a
   full clear/redraw and set `needs_refresh = true`.
4. Propagate the callback's `Result` (or resume-unwind the panic **after** the terminal is
   restored) so the caller can show an error popup.

This is the deferred "Phase 4" subprocess mechanism; it also unlocks `$EDITOR` commits and
interactive custom commands later. It lives in `src/gui/mod.rs` next to
`setup_terminal`/`restore_terminal`.

### 3. Review launcher (`src/gui/controller/review.rs`)

- A **pure** function mapping the active lazygitrs context + selection to tuicr `RunOptions`:

  | lazygitrs context | RunOptions |
  |---|---|
  | Files (whole working tree) | `working_tree = true` |
  | Files, single file selected | `working_tree = true`, `path_filter = Some(rel_path)` |
  | Commits / BranchCommits (selected commit `H`) | `revisions = Some(<single-commit diff>)` |
  | CommitFiles (file in commit `H`) | `revisions = Some(<single-commit diff>)`, `path_filter = Some(rel_path)` |

  **Revision form — verify against tuicr's parser.** tuicr resolves `revisions` via **git2**,
  which does not accept `git rev-list` shorthand like `H^!`. Use the range tuicr's VCS layer
  actually parses (likely `H~1..H` / `H^..H`), and handle the **root commit** (no parent)
  case explicitly. The exact string is an implementation detail to confirm against tuicr's
  revision handling, not assumed.

- The controller entry point (bound to `keybindings.*.review` = `<c-r>`):
  1. Build `RunOptions` from context; if the context is unsupported, no-op (or a brief info
     popup).
  2. Set current dir to `gui.git.repo_path()` for the duration (tuicr discovers the repo from
     cwd), then call `run_with_terminal_suspended(term, kbd, || tuicr::run_review(opts))`.
  3. On `Err`, show a lazygitrs error popup. Always `needs_refresh = true` afterward.

  All of component 3's tuicr-touching code is behind `#[cfg(feature = "review")]`; with the
  feature off, the keybinding shows a "built without review support" info popup (or is
  hidden).

## Data flow

```
key <c-r>  ->  review::launch(gui)
            ->  build RunOptions from active ContextId + selection
            ->  run_with_terminal_suspended:
                   restore lazygitrs terminal
                   set cwd = repo_path
                   tuicr::run_review(opts)   // tuicr sets up its own terminal + loop
                   (tuicr persists comments to its ReviewStore, optionally submits via gh)
                   restore lazygitrs terminal, force redraw
            ->  needs_refresh = true; show error popup if run_review returned Err
```

## Cargo / build

```toml
[features]
review = ["dep:tuicr"]

[dependencies]
tuicr = { git = "https://github.com/<user>/tuicr", branch = "embed-run", optional = true }
```

- `review` is **off by default**. `cargo build` stays lean; `cargo build --features review`
  pulls in tuicr → a second ratatui (0.30) + crossterm (0.29), git2 (enable its `vendored`
  feature to avoid a system libgit2 dependency), and syntect. Bigger, slower builds — call
  this out in the README install notes.
- Distribution artifacts (Homebrew/npm/nix/cargo-binstall) decide whether to ship
  `--features review`; document the trade-off. (Not blocking for v1 code.)

## Error handling & safety

- **Terminal always restored:** the handoff helper's `catch_unwind` + unconditional
  re-setup guarantee lazygitrs's terminal returns even if tuicr errors or panics.
- **tuicr error → popup:** `run_review` returning `Err` surfaces as a lazygitrs error popup;
  lazygitrs keeps running.
- **Unsupported context:** launching from a panel with no sensible target is a no-op / brief
  info popup, never a crash.
- **cwd:** restore the previous working directory after the review even on error.

## Testing

- **Unit (pure):** table tests for `context → RunOptions` over each supported `ContextId` and
  selection state, including the unsupported-context case.
- **Handoff helper:** test the state effects it can assert without a real TTY (that
  `needs_refresh` is set and a full-redraw is requested on both Ok and Err paths); the raw
  terminal calls are exercised manually.
- **Smoke (manual):** in a scratch repo, launch review from Files, Files+file, Commits, and
  CommitFiles; confirm the correct target loads in tuicr and that quitting returns to the
  same lazygitrs panel/selection.
- **Fork:** keep tuicr's own test suite green after the extract refactor; add one test that
  `run_review` with `working_tree` on a temp repo starts and exits cleanly (headless-friendly
  subset if feasible).

## Risks

- **Fork maintenance:** ~1-commit rebase per tuicr release (accepted; matches existing
  workflow). Mitigation: keep the extract patch minimal and offer it upstream.
- **git2 native build:** use `git2`'s `vendored` feature so the `review` build doesn't need a
  system libgit2.
- **Binary size / build time** when `review` is enabled (accepted; feature is opt-in).
- **tuicr internal API drift** (`App::new`, `RunOptions` fields): contained to the fork's
  extract patch plus the small launcher; surfaces at compile time, not runtime.

## Future (post-v1)

- PR review mode via `pr_target` (lazygitrs already uses `gh`).
- Multi-commit range review once lazygitrs gains multi-select.
- Data-level integration: read tuicr's `ReviewStore` to badge reviewed files/hunks in
  lazygitrs panels.
- Reuse the terminal-handoff helper for `$EDITOR` commits and interactive custom commands.

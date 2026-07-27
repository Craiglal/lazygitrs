# Editor integration — deferred follow-ups

Date: 2026-07-28
Source: final whole-branch review of `docs/superpowers/plans/2026-07-27-editor-integration.md`

These were found by per-task or final review, adjudicated, and deliberately **not** fixed in that
work. They are recorded here because the review workspace they were tracked in
(`.superpowers/sdd/`) is git-ignored scratch and evaporates.

Everything the final review triaged as *fix-before-merge* was fixed and is **not** listed here.

## Follow-up

**1. Signal number missing from the editor-death message.**
`src/gui/interactive.rs` — the signal arm reports `"editor was killed by a signal"` without saying
which. `std::os::unix::process::ExitStatusExt::signal()` is dependency-free. This is the arm a user
hits when the editor segfaults or is `kill -9`'d, so the number is the entire diagnostic. Needs a
`#[cfg(unix)]` guard.

**2. `run()`'s second `restore_terminal` can swallow the error it exists to report.**
`src/gui/mod.rs` — `restore_terminal(&mut terminal, keyboard_enhanced)?;` after `main_loop` returns.
On the `EditError::Terminal` path `main_loop` returns the real cause, and this `?` can replace it
with a second, less informative failure. Masks *any* `main_loop` error, not only this one.
`let _ = restore_terminal(..);` preserves `result`; losing a restore error is harmless because the
panic hook and the shell both cover that case.

**3. `e` advertised in the `BranchCommits` help section where it does nothing.**
`src/gui/mod.rs` — the combined `ContextId::BranchCommits | ContextId::BranchCommitFiles` help arm
lists `e Open in editor`, but `branch_commits::handle_key` handles only `Esc`/`Enter`/`o`/`y`
(confirmed by a scoped `audit-help` run). `e` on a *commit* has no meaning, so the fix is
presentational — split the arm, or qualify the description as `"Open in editor (file list)"`. That
section already carries other context-conditional entries (`<enter> View commit files`,
`<esc> Back to branches` are both `BranchCommits`-only), so this follows an existing flaw.

**4. `bbedit` preset omits `--`.**
`src/os/editor.rs` — upstream lazygit's `bbedit` templates include `--`; ours do not. Only matters
for a filename starting with `-`, which shell quoting cannot rescue because quoting does not stop
option parsing. Free to add if BBEdit accepts it.

**5. `git config core.editor` is read from the wrong repository.**
`src/os/editor.rs` — `git_core_editor()` runs `git` in the *process* CWD and `ENV_CANDIDATES` is a
process-wide `Lazy`. So `lazygitrs -p /other/repo` reads whatever repo the shell was in, and after an
in-session repo switch (`request_repo_open`, used for worktrees/submodules) the first repo's value
sticks for the session. Low impact — `core.editor` is nearly always global. Fix by passing
`repo_path()` as `current_dir` and keying the memo on it.

**6. Doc comment on `usable_editor_value` overclaims.**
`src/os/editor.rs` — it says the whole value is preserved "so `EDITOR="nvim -u NONE"` keeps its
arguments". True of that function, false of the resolution *outcome*: when a preset matches, the
preset's template is used and the flags are dropped (correct, matches lazygit, and pinned by
`preset_for_editor_string_matches_a_value_carrying_arguments`). Add a clause so nobody concludes
`EDITOR="code --wait"` keeps `--wait`.

**7. `o` in the diff panel discards its error** while `o` in the Files panel propagates it.
`src/gui/mod.rs` vs `src/gui/controller/files.rs`. Pre-existing, but in scope by proximity — the
`run_template` they share was rewritten by this work, and it is the same silent-failure class.

**8. `run_edit_request`'s three-way dispatch is the one untested seam.**
Inverting the `Some(cmd) if cmd.suspend` guard — terminal editors detached, GUI editors suspending —
passes the whole suite. Extracting the choice as a pure
`fn plan_edit(os, line) -> EditPlan { Suspend(..), Detached(..), DefaultProgram }` would make it
testable. Optional; a prior review correctly rejected the heavier `suspend_around(..)` extraction
because it would only regression-lock behaviour.

**8b. A second detached edit drops the first `Child` unreaped.**
`src/gui/mod.rs` — the drain does `Ok(detached) => self.detached_editor = detached`, overwriting any
in-flight entry. If a user triggers a second detached (GUI) edit before the main loop has reaped the
first, that first `Child` is dropped without `wait()`, leaking one zombie.

Introduced by the final fix wave's own prescribed shape, and flagged by the implementer rather than
silently improvised around — the right call. Parked rather than fixed because it is strictly better
than the pre-fix behaviour (where *every* detached edit leaked, unconditionally) and the window is
narrow: the loop reaps once per ~16ms iteration, and most GUI launchers (`code --reuse-window`) fork
and exit immediately, so the entry is normally gone before a second press is possible. It needs an
editor that genuinely blocks (`code --wait`) plus a second press inside one iteration.

Fix when convenient: reap-then-replace (`if let Some(mut prev) = self.detached_editor.take() { let _ =
prev.into_child().wait(); }`) or hold a small `Vec<DetachedEditor>` and drain it each iteration.

## Won't fix (with reasoning)

**9. `acme` preset invokes `acme` directly** where lazygit shells out to `B` to reuse a running
instance. The reviewer who raised it was explicitly low-confidence, Plan 9 is not a target platform,
and a wrong guess is worse than the current honest one.

**10. Restricting the non-zero-exit popup to status 127.** vim's deliberate `:cq` (exit 1) raises a
popup on every use, which is a real annoyance. Keeping the broad behaviour anyway: 127 covers only
`sh`'s command-not-found, whereas a failing wrapper script, an editor that cannot write the file, or
an `os.edit` that mangles its own arguments all exit non-zero with some *other* code and would go
silent again — reintroducing this project's central defect class through a narrower door. If it bites
in practice the right knob is an explicit opt-out (`os.editIgnoreExitCode`), not a hardcoded filter.

**11. `os_config_reads_lazygit_editor_keys` cannot detect wire-name drift.** Its YAML fixture and the
`#[serde(rename)]` share the same string, so it proves self-consistency, not compatibility. Inherent
to unit-testing an external project's schema without fetching it. The fixture cites lazygit's
`docs/Config.md` instead, moving the check to review — which is where it actually worked: a reviewer
caught `suspendOnEdit` vs `editInTerminal` by reading lazygit's source.

**12. Windows support.** `sh -c` is unconditional in the execution layer, yet `OsConfig::default()`
still has a `cfg!(target_os = "windows")` arm producing `start "" {{filename}}`. That arm was already
broken (`start` is a `cmd.exe` builtin, not an executable), so this is not a regression, and the new
failure at least surfaces as a popup rather than nothing. Recorded so the Windows default is not read
as a support claim.

## Outstanding verification (human, cannot be automated)

`cargo test` has no TTY, so **no** automated gate covers terminal behaviour. The full per-context
checklist is in the final review; the highest-value items:

- `e` in each of the six contexts with `$EDITOR=nvim`; confirm nvim owns the TTY, opens at the right
  hunk, and the TUI redraws with no stale cells on `:q`.
- Repeated suspend cycles (five or six `e` → `:q`), then quit lazygitrs and check the shell: raw mode
  off, mouse reporting off, `Esc` normal — i.e. the keyboard-enhancement flag stack has not drifted.
  One clean cycle does not prove the balance.
- Resize the terminal *while* the editor is open, then quit — exercises the `terminal.size()`
  re-sync, which has no other coverage.
- `Ctrl-Z` inside the editor, then `fg`. `sh`/nvim share lazygitrs's process group, so `SIGTSTP` hits
  all three. This is the most plausible real-world route to a wrecked terminal and **nothing in the
  design addresses it** — if it misbehaves, that is a new finding, not a known one.
- A real lazygit config: `os: { editPreset: nvim, editInTerminal: false }` in
  `~/.config/lazygit/config.yml`, confirming it takes effect. This is the claim item 11 above cannot
  make in a unit test.

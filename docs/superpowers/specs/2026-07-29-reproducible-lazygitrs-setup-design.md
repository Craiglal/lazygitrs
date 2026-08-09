# Reproducible lazygitrs setup

**Date:** 2026-07-29
**Status:** Approved, pending implementation plan

## Problem

AI commit generation was broken on this machine. Root cause, confirmed by reproduction:
`git.commit.generateCommand` pointed at `/home/oleksii-luppa/.local/bin/ai-lazygitrs-commit`
— a stale home directory (`oleksii-luppa`; the real home is `oleksii`) — and no such script
existed under the real home either. Running it produced **exit 127, "No such file or
directory"**, which lazygitrs surfaces as `Generate command failed`.

Two failures compounded:

1. **A hand-copied config carried an absolute path containing a username.** Nothing on this
   machine is version-controlled outside project repos — no dotfiles repo, no chezmoi/stow —
   so the config was copied by hand between machines and the stale path survived silently.
2. **The helper script it referenced was never distributed at all.** It lived only on the old
   machine.

The same config also references `delta` (as a pager) and `lazyworktree` (a custom command),
neither of which is installed here. Same cause.

A main-PC reinstall on CachyOS is planned, so this must not recur.

## Goal

Fresh CachyOS box → clone the repo → two commands → fully working lazygitrs, AI commit
included.

```
make install     # cargo install --path .  (already exists)
make setup       # wire up helper + config, then report missing deps
make setup-deps  # opt-in, only when setup reports something missing
```

Scope is **lazygitrs and everything its config references** — not a general dotfiles system
for zsh/fish/tmux.

## Design

### Components

**`contrib/ai-commit`** — commit-message helper for the DeepSeek API, generalized from the
working version validated on 2026-07-29.

- Reads `git diff --cached` on **stdin**. This is required: `GenerateInputMode::for_command`
  (`src/git/ai_commit.rs:142`) only selects repo-inspection mode for command names
  `claude|codex|crabcode|opencode`; every other name gets `StdinDiff`.
- Writes the commit message to stdout, nothing else. Actionable reason to stderr on failure
  (lazygitrs displays stderr when the command exits non-zero).
- Env-tunable: `AI_COMMIT_MODEL` (default `deepseek-v4-flash`), `AI_COMMIT_API_URL`,
  `AI_COMMIT_MAX_TOKENS` (default 3000), `AI_COMMIT_TIMEOUT`, `AI_COMMIT_MAX_DIFF_CHARS`,
  `AI_COMMIT_REASONING_EFFORT` (default `none`; empty omits the field and takes the API
  default).
- Requires `jq` and `curl`; reads `DEEPSEEK_API_KEY` from the environment.

**`contrib/config.yml`** — the canonical, version-controlled config; target of the config
symlink. **Contains no machine-specific absolute paths.** It references
`$HOME/.local/bin/ai-lazygitrs-commit`; lazygitrs runs `generateCommand` through `sh -c`, so
`$HOME` expands correctly.

### Reasoning-model handling (non-obvious, load-bearing)

`deepseek-v4-flash` is a reasoning model: reasoning tokens are drawn from the same
`max_tokens` budget as the response. A probe with `max_tokens: 20` returned
`content: ""` with `reasoning_tokens: 20` — the entire budget consumed by reasoning, leaving
an empty message. A naive helper fails here with lazygitrs' unhelpful "produced no commit
message".

The helper therefore:

- reads `.choices[0].message.content`, **not** `.reasoning_content`;
- sends `reasoning_effort: "none"` by default. Amended 2026-08-03: budgeting 3000 tokens for
  "reasoning plus message" does not hold, because reasoning grows with the diff while the
  budget does not — a 10 KB diff spent 1930 of 3000 tokens reasoning, and larger diffs
  returned an empty or truncated message. Writing a commit message needs no chain of thought,
  so the whole budget now goes to the message: the same 10 KB diff costs 227 completion
  tokens. `AI_COMMIT_REASONING_EFFORT` opts reasoning back in;
- when `finish_reason == "length"`, reports which limit was hit — reasoning eating the budget
  (and offers `AI_COMMIT_REASONING_EFFORT=none`), an empty message with reasoning already off,
  or a message truncated mid-way — and names `AI_COMMIT_MAX_TOKENS` as the fix. A truncated
  message is a failure, never passed through to the commit editor.

Verified against the live API: only `deepseek-v4-flash` and `deepseek-v4-pro` exist.

### Two symlinks

```
~/.local/bin/ai-lazygitrs-commit  ->  <checkout>/contrib/ai-commit
~/.config/lazygitrs/config.yml    ->  <checkout>/contrib/config.yml
```

The helper gets its own symlink into `~/.local/bin` rather than having the config point into
the checkout directly. If `generateCommand` named a checkout path, a fresh box that clones
somewhere else would break — the same class of bug as the original failure. Routing through a
fixed `$HOME`-relative path makes the config independent of checkout location.

Naming: the repo file is `contrib/ai-commit` (unprefixed — the repo already provides the
context), while the symlink is `ai-lazygitrs-commit` because `~/.local/bin` is a global
namespace. The symlink name also matches what the existing config already used.

Symlinking `config.yml` into the repo is safe: lazygitrs never writes it. Runtime toggles
persist to `state.yml` (`_docs/persisted-settings.md`), and `Config::save_state`
(`src/config/mod.rs:74`) only ever writes the state path.

Because `~/.config/lazygitrs/config.yml` is the first existing candidate directory
(`src/config/mod.rs:20`), the symlink is what gets loaded.

### `make setup`

Idempotent, no sudo, safe to re-run:

1. Create `~/.local/bin` and `~/.config/lazygitrs` if absent.
2. Symlink the helper. If a **regular file** already exists at the target (as it does on this
   machine today), move it to `<name>.bak` and replace it with the symlink.
3. Symlink the config, with the same clobber rule: a real `config.yml` is moved to
   `config.yml.bak` and never silently overwritten. An already-correct symlink is a no-op.
4. Run the `doctor` checks and print the report.

`setup` exits 0 whenever **its own** work succeeded, even if the report lists missing
dependencies. On a fresh box `jq` or `delta` will legitimately be absent until `setup-deps`
runs, and `make install setup` must not appear to fail at that point. Only a failure to create
the symlinks makes `setup` exit non-zero. `make doctor` is the target with strict exit codes.

### `make doctor`

Read-only; changes nothing; exits non-zero if any required check fails (so it can gate CI).

- `lazygitrs` on PATH, and its version.
- `~/.config/lazygitrs/config.yml` resolves, and is the expected symlink.
- **`generateCommand` resolves to an executable** — parse the value from the config, expand
  it, take the first word, and confirm it exists and is executable. This is the check that
  would have diagnosed the original bug in one shot.
- Required deps present: `jq`, `curl`.
- Optional deps present: `delta`, `lazyworktree`.
- `DEEPSEEK_API_KEY` is set — **presence only, never printed**. Checked only when
  `generateCommand` resolves to the bundled helper. **Amended after the final
  review:** the original wording made this required for *any* configured
  `generateCommand`, which hard-failed the `claude`, `opencode`, `codex exec`, and
  `modelcli` options the README advertises. A DeepSeek key is irrelevant to those,
  so gating on the bundled helper is the correct condition.

Exit-code classification, stated explicitly to avoid two readings:

| Check | Missing → |
|---|---|
| `lazygitrs`, `config.yml`, `generateCommand` resolves, `jq`, `curl` | failure (non-zero) |
| `DEEPSEEK_API_KEY` | failure **only if** `generateCommand` resolves to the bundled helper (`…ai-lazygitrs-commit`); otherwise skipped |
| `delta`, `lazyworktree` | warning (still exits 0) |

### `make doctor-ai`

Opt-in, separate target because it costs a real API call. Runs one end-to-end generation
against a small synthetic diff to prove key + model + network actually work. Presence of a key
is not proof it is valid: a live probe during investigation returned
`HTTP 401: Authentication Fails`. `doctor` alone cannot catch that.

### `make setup-deps`

Opt-in, never invoked by `setup`. Detects `paru`, then `yay`, then `pacman`, and installs
`jq`, `git-delta`, and `lazyworktree` (AUR). Aborts with a clear message if no supported
package manager is found.

## Error handling

- `setup` never destroys an existing real file; it backs up and warns.
- The helper gives one actionable line per failure mode. All four verified on 2026-07-29:
  - no staged changes → `No staged changes to describe.`
  - missing key → names the variable and where it is expected from
  - bad key → `DeepSeek API returned HTTP 401: Authentication Fails`
  - starved budget → names `AI_COMMIT_MAX_TOKENS` as the fix
- Amended 2026-08-03: the blank-input and blank-message checks are shell `case` matches, not
  `printf … | grep -q`. Under `set -o pipefail`, `grep -q` exits at the first match while
  `printf` is still writing, and that SIGPIPE became the pipeline's status once the diff
  outgrew the 64 KiB pipe buffer — so a large staged diff was reported as `No staged changes
  to describe.`
- `doctor` distinguishes required failures (non-zero exit) from optional-dep warnings.

## Testing

Add `AI_COMMIT_STUB_RESPONSE` — a path to a canned JSON file that short-circuits the `curl`
call — and `contrib/test-ai-commit.sh` driving four cases offline and deterministically:
empty diff, API error, empty-content-from-reasoning, and happy path. This keeps the failure
paths tested without network access in CI.

This is the only piece of added surface in the design and was flagged as such at approval
time. If it proves awkward, drop it and rely on `make doctor-ai` plus manual checks.

Existing Rust tests in `src/git/ai_commit.rs` remain the coverage for lazygitrs' own
fence-stripping and input-mode logic; this spec does not change that code.

## Documentation

Add a DeepSeek example to the `generateCommand` list in `README.md`, referencing
`contrib/ai-commit`, then run `make sync-readme` (`make readme-check` gates on the copy in
`npm/README.md` matching).

## Out of scope

- **Secret syncing.** `DEEPSEEK_API_KEY` stays in `~/.zshenv`. Setup prints the line to add
  and never stores the value. Reproducing secrets across machines is a broader unsolved
  problem here and deliberately not addressed.
- Native HTTP / API-key handling inside the Rust binary. `generateCommand` stays the seam.
- A general dotfiles framework for shells, tmux, or editors.

## Accepted trade-off

`contrib/config.yml` doubles as the example config users see, so personal preferences (the
`delta` pager arguments, the `lazyworktree` custom command) ship in the public repo. It
contains no secrets, and `make setup` is opt-in for users. Confirmed acceptable at approval.

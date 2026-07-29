# Reproducible lazygitrs setup — deferred follow-ups

Date: 2026-07-29
Source: per-task and final whole-branch review of
`docs/superpowers/plans/2026-07-29-reproducible-lazygitrs-setup.md`

These were found by per-task or final review, adjudicated, and deliberately **not** fixed in that
work. They are recorded here because the review workspace they were tracked in
(`.superpowers/sdd/`) is git-ignored scratch and evaporates.

Everything the final review triaged as *fix-before-merge* was fixed and is **not** listed here.

## Substantial

**1. `git.pagers` is not implemented, and `git.paging.useConfig` is dead.**
`contrib/config.yml` carries a `git.pagers` block configuring `delta`. `GitConfig`
(`src/config/user_config.rs`) has no `pagers` field — only `paging: PagingConfig`, whose sole
member is `useConfig: bool` — and there is no `deny_unknown_fields`, so the block is silently
dropped at load time. The delta pager it appears to configure has therefore never applied.
Separately, `paging.use_config` is itself never read anywhere in `src/`: lazygitrs has no external
pager support at all and renders diffs through its own `src/pager/` module.

The human ruled that the config governs and the **code** should be brought up to match, rather than
dropping the block. That work needs its own spec: a `GitConfig.pagers` field (a list of
`{colorArg, pager}`), pager selection honouring `colorArg` (delta needs `--color=always` when git
is not writing to a TTY), and wiring into `src/pager/` rendering. Decide there whether
`paging.useConfig` should be implemented or deleted.

Useful reference for scope: the author's own git config already drives delta for `pager.log`,
`pager.show`, and `pager.blame`, with `delta.side-by-side`, `line-numbers`, `hyperlinks`,
`navigate`, and a `syntax-theme`.

**2. Custom commands cannot bind a key that `handle_key` already claims, and the collision is
terminal-dependent.**
The original `contrib/config.yml` bound `W` to `lazyworktree create`. `src/gui/mod.rs:2425` handles
`KeyCode::Char('W')` for diff/compare mode and returns at 2428, while custom-command dispatch is at
2510 — so the binding never fired. It was rebound to `K` in this work, which resolves the instance
but not the class.

Worse than uniformly dead: `key_matches` (`src/config/keybindings.rs`) accepts `Char('w')+SHIFT` for
the binding `"W"`, while the built-in arm matches only `Char('W')`. On terminals that report shift+w
as lowercase+SHIFT, the custom command *does* fire. So the same config behaves differently across
terminals.

Worth deciding deliberately: either consult custom commands before built-ins (risking user configs
shadowing built-in keys), or validate at config-load time and warn that a binding is unreachable.
The silent, terminal-dependent middle ground is the worst of the three.

**3. No CI workflow runs the tests.**
The repo has only cargo-dist's `release.yml`. `cargo test` and `test-contrib` therefore run solely
when a human types `make ci` or `make preflight`. The spec for this work describes `doctor` exiting
non-zero "so it can gate CI", which is aspirational until a workflow exists.

## Minor

**4. `contrib/`'s platform assumptions are unstated.**
The scripts assume GNU sed (`contrib/ai-commit`'s blank-line trim uses `N;ba`, which BSD/macOS sed
rejects) and Arch package management (`contrib/setup-deps.sh` knows only `pacman`/`paru`/`yay`). The
project ships to macOS through Homebrew and npm, so a macOS user will find these and be misled.
Worth one statement at the top of the directory.

**5. `assert_contains` matches its needle as a shell glob.**
`contrib/test-lib.sh` uses `case "$haystack" in *"$needle"*)`, so a needle containing `*`, `?`, or
`[` would match as a pattern. No current needle contains a glob metacharacter, so it is benign
today.

**6. `doctor.sh`'s shell-expression guard misses backticks.**
It detects `$(`, `&&`, `|`, and `;` but not `` ` ``. A backtick-using `generateCommand` falls
through to the plain-command branch and is likely reported as a bad path. Nothing executes it, so
the worst case is a cosmetically wrong mark. Note this becomes load-bearing if anyone ever
reintroduces an `eval`-based resolution — which was explicitly rejected during this work.

**7. `doctor.sh` cannot read a YAML block-scalar `generateCommand`.**
Its extraction handles only the single-line `generateCommand: <value>` form; a value on a following
indented line yields "no generateCommand configured". Documented in the script's own comment.

**8. `doctor.sh` does not expand the `~user` form.**
Bare `~/` is expanded; `~someone/` is not, and would fail resolution.

**9. `AI_COMMIT_MAX_DIFF_CHARS` is not validated.**
A non-numeric value prints a raw bash `[: abc: integer expression expected` to stderr, skips
truncation, and still exits 0. Harmless because lazygitrs ignores stderr on success.
`AI_COMMIT_MAX_TOKENS` fails cleanly through `jq` instead.

**10. `AI_COMMIT_API_URL` silently replaced `DEEPSEEK_API_URL`.**
The pre-branch helper read the old name. Anyone who had exported it loses effect with no signal.

**11. `AI_COMMIT_TIMEOUT` and `AI_COMMIT_API_URL` have no test coverage.**
Both are straight passthroughs to `curl`. `MAX_DIFF_CHARS` was exercised by hand during the final
review and works.

**12. Small cosmetic items.**
`contrib/setup-deps.sh` interpolates `$0` rather than `$(basename "$0")` in its usage message, and
its pacman-only path does not restate that only `lazyworktree` remains uninstalled (both
`setup-deps.sh` and `doctor.sh` do self-report this at runtime). `contrib/setup.sh`'s
existing-file branch also matches a *directory* at the link path, and its wrong-symlink branch has a
brief `rm -f`-then-`ln -s` window; neither risks data. A *dangling* symlink at a candidate backup
name tests `-e` false and would be overwritten by `mv`, but holds no content, so the never-clobber
guarantee is intact.

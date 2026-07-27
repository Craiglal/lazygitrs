# Editor Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `e` open the selected file in the user's terminal editor (nvim), with the editor owning the terminal, working out of the box from `$EDITOR`.

**Architecture:** A new pure module `src/os/editor.rs` resolves *which* command to run (explicit config → named preset → `$VISUAL`/`$EDITOR`/`git config core.editor`). A new module `src/gui/interactive.rs` holds the request type and the terminal-handoff helper. Key handlers never touch the terminal: they set `gui.pending_interactive`, and `main_loop` — which owns the `Terminal` — drains it, suspends the TUI, runs the editor to completion, rebuilds the terminal, and refreshes.

**Tech Stack:** Rust 2021, ratatui 0.29, crossterm 0.28, anyhow, serde/serde_yaml, once_cell. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-07-27-editor-integration-design.md`

## Global Constraints

- No new crate dependencies. `once_cell` (`Cargo.toml:23`) is already available; use it for memoisation.
- YAML config keys must match lazygit exactly: `editPreset`, `suspendOnEdit`. `src/config/mod.rs:20` falls back to `~/.config/lazygit/config.yml`, so an existing lazygit config must keep working.
- `$GIT_EDITOR` must **never** be consulted. It is commonly set to the no-op binary `true` (it is on the target machine), which would make `e` a silent no-op. The literal values `true`, `false`, and `:` must be rejected wherever they appear in the detection chain.
- Editor commands run through `sh -c`. `{{filename}}` is always POSIX single-quote escaped; `{{line}}` and `{{column}}` are plain digits.
- A non-zero editor exit status must surface as an error popup. `sh -c` turns *command not found* into a successful spawn with exit status 127, so swallowing non-zero statuses would hide a mistyped `os.edit` entirely.
- Terminal rebuild after suspension must happen on **every** exit path, including panics, before the outcome is inspected.
- Test command is plain `cargo test` — `lazygitrs` is a **binary-only** crate, so `cargo test --lib` fails with
  "no library targets found". Filter with a module path instead: `cargo test editor::`.
- The repo carries roughly 48 pre-existing warnings (dead code, unused imports) at baseline. Do **not** try to
  fix them — they are out of scope. Build gates check for `error` only. A `never used` warning for a function
  this task adds is expected until a later task wires it up.
- Do not touch `os.editAtLineAndWait` or `os.openDirInEditor`. Both fields exist and are unread; they stay unread.
- No README changes (the README documents no config keys). If that ever changes, `just sync_readme` is mandatory per `CLAUDE.md`.

---

### Task 1: Shell quoting and template expansion

The foundation both the editor path and the existing `os.open` path need. `OsConfig::run_template` currently does `split_whitespace()` on the *substituted* string (`src/config/user_config.rs:270-279`), which mangles any path containing a space.

**Files:**
- Create: `src/os/editor.rs`
- Modify: `src/os/mod.rs` (2 lines total today: `pub mod cmd;` / `pub mod platform;`)
- Test: inline `#[cfg(test)] mod tests` in `src/os/editor.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub fn shell_quote(s: &str) -> String`
  - `pub fn expand(template: &str, path: &str, line: Option<usize>, column: usize) -> String`

- [ ] **Step 1: Register the module**

In `src/os/mod.rs`, add the line so the list reads:

```rust
pub mod cmd;
pub mod editor;
pub mod platform;
```

- [ ] **Step 2: Write the failing tests**

Create `src/os/editor.rs` containing *only* the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_wraps_plain_path() {
        assert_eq!(shell_quote("/tmp/a.rs"), "'/tmp/a.rs'");
    }

    #[test]
    fn shell_quote_preserves_spaces() {
        assert_eq!(shell_quote("/my repo/a b.rs"), "'/my repo/a b.rs'");
    }

    #[test]
    fn shell_quote_escapes_embedded_single_quote() {
        // Closing quote, an escaped literal quote, then reopening.
        assert_eq!(shell_quote("/tmp/don't.rs"), "'/tmp/don'\\''t.rs'");
    }

    #[test]
    fn shell_quote_handles_empty_and_non_ascii() {
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("/tmp/тест.rs"), "'/tmp/тест.rs'");
    }

    #[test]
    fn expand_substitutes_all_placeholders() {
        assert_eq!(
            expand(
                "code --goto -- {{filename}}:{{line}}:{{column}}",
                "/tmp/a.rs",
                Some(12),
                5,
            ),
            "code --goto -- '/tmp/a.rs':12:5"
        );
    }

    #[test]
    fn expand_substitutes_filename_more_than_once() {
        assert_eq!(
            expand("cp {{filename}} {{filename}}.bak", "/tmp/a.rs", None, 1),
            "cp '/tmp/a.rs' '/tmp/a.rs'.bak"
        );
    }

    #[test]
    fn expand_leaves_no_placeholder_when_line_is_unknown() {
        let out = expand("vim +{{line}} -- {{filename}}", "/tmp/a.rs", None, 1);
        assert_eq!(out, "vim +1 -- '/tmp/a.rs'");
        assert!(!out.contains("{{"), "placeholder leaked: {out}");
    }

    #[test]
    fn expand_passes_through_template_without_placeholders() {
        assert_eq!(expand("true", "/tmp/a.rs", None, 1), "true");
    }

    #[test]
    fn expand_does_not_rewrite_placeholders_inside_the_path() {
        assert_eq!(
            expand("vim +{{line}} -- {{filename}}", "/tmp/{{line}}.rs", Some(5), 1),
            "vim +5 -- '/tmp/{{line}}.rs'"
        );
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test editor::`
Expected: FAIL to compile — `cannot find function shell_quote in this scope`.

- [ ] **Step 4: Write the implementation**

Prepend to `src/os/editor.rs`, above the test module:

```rust
//! Resolves which editor command to run for an "open in editor" action, and
//! expands its command template into a shell command line.

/// POSIX single-quote escaping: wrap in single quotes and rewrite any embedded
/// single quote as `'\''` (close, escaped literal quote, reopen). Safe for any
/// byte sequence a path can hold.
pub fn shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Expand `{{filename}}` (shell-quoted), `{{line}}` and `{{column}}` into a
/// command line suitable for `sh -c`.
///
/// When `line` is `None` the `{{line}}` placeholder becomes `1`, so a literal
/// placeholder can never leak into the command line even if a caller pairs a
/// line-aware template with an unknown line.
pub fn expand(template: &str, path: &str, line: Option<usize>, column: usize) -> String {
    // `{{filename}}` is substituted last, and `str::replace` never re-scans the
    // text it inserts, so a path that itself contains `{{line}}` or `{{column}}`
    // cannot be corrupted by a later pass.
    template
        .replace("{{line}}", &line.unwrap_or(1).to_string())
        .replace("{{column}}", &column.to_string())
        .replace("{{filename}}", &shell_quote(path))
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test editor::`
Expected: PASS, 9 tests.

- [ ] **Step 6: Commit**

```bash
git add src/os/editor.rs src/os/mod.rs
git commit -m "feat(os): add shell quoting and editor template expansion"
```

---

### Task 2: Route `run_template` through `sh -c`

Fixes the `split_whitespace()` bug for the `os.open` / "open in default program" paths, which stay non-blocking.

**Files:**
- Modify: `src/config/user_config.rs:263-307` (the whole `impl OsConfig` block)
- Test: inline `#[cfg(test)] mod tests` in `src/config/user_config.rs` (the file has none today — add one at the end)

**Interfaces:**
- Consumes: `crate::os::editor::expand` from Task 1.
- Produces: `OsConfig::run_template` and `OsConfig::run_template_at_line` keep their existing signatures and their non-blocking `.spawn()` behaviour. Task 8 deletes `run_template_at_line` once its last caller is gone.

- [ ] **Step 1: Write the failing test**

Append to `src/config/user_config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_template_rejects_an_empty_template() {
        let err = OsConfig::run_template("", "/tmp/a.rs").unwrap_err();
        assert!(
            err.to_string().contains("No command configured"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn run_template_at_line_rejects_an_empty_template() {
        let err = OsConfig::run_template_at_line("", "/tmp/a.rs", 3, 1).unwrap_err();
        assert!(
            err.to_string().contains("No command configured"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn run_template_quotes_a_path_containing_a_space_as_one_shell_word() {
        // Regression: the old implementation ran split_whitespace() on the
        // substituted string, shredding this path into three arguments. The
        // command log records the expanded line, so asserting on it fails if
        // anyone reverts the quoting. Asserting that run_template merely
        // returns Ok() does NOT work: `true` ignores its argv and .spawn()
        // never inspects argument count, so such a test passes under the bug.
        let log = crate::os::cmd::new_command_log();
        crate::os::cmd::set_thread_command_log(log.clone());
        OsConfig::run_template("true {{filename}}", "/my repo/a b.rs").unwrap();
        let entries = log.lock().unwrap();
        assert_eq!(entries.last().unwrap().as_str(), "true '/my repo/a b.rs'");
    }
}
```

- [ ] **Step 2: Run the tests to verify the third one is the meaningful check**

Run: `cargo test user_config::`
Expected: the two empty-template tests PASS already (those guards exist). The command-log test FAILS against the current `split_whitespace()` implementation, which logs the unquoted `true /my repo/a b.rs` — that failure is the red phase for Step 3.

- [ ] **Step 3: Rewrite both methods**

Replace the bodies of `run_template` and `run_template_at_line` in `src/config/user_config.rs` (currently lines 266-306). Keep the doc comments, updating them as shown:

```rust
impl OsConfig {
    /// Run a command template, replacing `{{filename}}` with the given path.
    /// Runs via `sh -c` and does **not** wait: this is the "open in default
    /// program" path, which must never block the TUI.
    /// If the template is empty, returns an error.
    pub fn run_template(template: &str, filename: &str) -> anyhow::Result<()> {
        if template.is_empty() {
            anyhow::bail!("No command configured");
        }
        let cmd_str = crate::os::editor::expand(template, filename, None, 1);
        if cmd_str.trim().is_empty() {
            anyhow::bail!("Empty command after template expansion");
        }
        crate::os::cmd::log_command(&cmd_str);
        std::process::Command::new("sh")
            .args(["-c", &cmd_str])
            .spawn()?;
        Ok(())
    }

    /// Run a command template replacing `{{filename}}`, `{{line}}`, and
    /// `{{column}}` with the given values. Non-blocking, like `run_template`.
    pub fn run_template_at_line(
        template: &str,
        filename: &str,
        line: usize,
        column: usize,
    ) -> anyhow::Result<()> {
        if template.is_empty() {
            anyhow::bail!("No command configured");
        }
        let cmd_str = crate::os::editor::expand(template, filename, Some(line), column);
        if cmd_str.trim().is_empty() {
            anyhow::bail!("Empty command after template expansion");
        }
        crate::os::cmd::log_command(&cmd_str);
        std::process::Command::new("sh")
            .args(["-c", &cmd_str])
            .spawn()?;
        Ok(())
    }
}
```

- [ ] **Step 4: Run the tests and build**

Run: `cargo test user_config:: && cargo build`
Expected: 3 tests PASS, build succeeds with no warnings about the module.

- [ ] **Step 5: Commit**

```bash
git add src/config/user_config.rs
git commit -m "fix(config): run os templates via sh -c so paths with spaces work"
```

---

### Task 3: Editor preset table and matching

**Files:**
- Modify: `src/os/editor.rs` (add above the existing test module; extend the test module)

**Interfaces:**
- Consumes: `shell_quote` / `expand` from Task 1 (not directly used here, same module).
- Produces:
  - `pub struct Preset { pub name: &'static str, pub aliases: &'static [&'static str], pub edit: &'static str, pub edit_at_line: &'static str, pub suspend: bool }`
  - `pub fn preset_by_name(name: &str) -> Option<&'static Preset>`
  - `pub fn usable_editor_value(value: &str) -> Option<&str>`
  - `pub fn preset_for_editor_string(value: &str) -> Option<&'static Preset>`

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `src/os/editor.rs`:

```rust
    #[test]
    fn preset_by_name_finds_canonical_names() {
        assert_eq!(preset_by_name("nvim").unwrap().name, "nvim");
        assert_eq!(preset_by_name("vscode").unwrap().name, "vscode");
    }

    #[test]
    fn preset_by_name_finds_aliases_and_ignores_case() {
        assert_eq!(preset_by_name("hx").unwrap().name, "helix");
        assert_eq!(preset_by_name("code").unwrap().name, "vscode");
        assert_eq!(preset_by_name("NVim").unwrap().name, "nvim");
    }

    #[test]
    fn preset_by_name_rejects_unknown_names() {
        assert!(preset_by_name("definitely-not-an-editor").is_none());
        assert!(preset_by_name("").is_none());
    }

    #[test]
    fn terminal_presets_suspend_and_gui_presets_do_not() {
        assert!(preset_by_name("nvim").unwrap().suspend);
        assert!(preset_by_name("nano").unwrap().suspend);
        assert!(!preset_by_name("vscode").unwrap().suspend);
        assert!(!preset_by_name("zed").unwrap().suspend);
    }

    #[test]
    fn preset_for_editor_string_matches_a_bare_binary() {
        assert_eq!(preset_for_editor_string("nvim").unwrap().name, "nvim");
    }

    #[test]
    fn preset_for_editor_string_matches_an_absolute_path() {
        assert_eq!(
            preset_for_editor_string("/usr/local/bin/hx").unwrap().name,
            "helix"
        );
    }

    #[test]
    fn preset_for_editor_string_matches_a_value_carrying_arguments() {
        assert_eq!(preset_for_editor_string("nvim -u NONE").unwrap().name, "nvim");
    }

    #[test]
    fn preset_for_editor_string_rejects_no_op_binaries() {
        // Regression: $GIT_EDITOR is commonly `true`. Launching it would make
        // `e` exit 0 having done nothing — a silent failure.
        assert!(preset_for_editor_string("true").is_none());
        assert!(preset_for_editor_string("false").is_none());
        assert!(preset_for_editor_string(":").is_none());
        assert!(preset_for_editor_string("/usr/bin/true").is_none());
        assert!(preset_for_editor_string("").is_none());
        assert!(preset_for_editor_string("   ").is_none());
    }

    #[test]
    fn usable_editor_value_keeps_the_whole_invocation() {
        assert_eq!(usable_editor_value("nvim -u NONE"), Some("nvim -u NONE"));
        assert_eq!(usable_editor_value("  subl  "), Some("subl"));
    }

    #[test]
    fn usable_editor_value_rejects_no_op_binaries() {
        assert_eq!(usable_editor_value("true"), None);
        assert_eq!(usable_editor_value("/bin/false"), None);
        assert_eq!(usable_editor_value(""), None);
    }

    #[test]
    fn usable_editor_value_accepts_editors_with_no_preset() {
        assert_eq!(usable_editor_value("my-weird-editor"), Some("my-weird-editor"));
        assert!(preset_for_editor_string("my-weird-editor").is_none());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test editor::`
Expected: FAIL to compile — `cannot find function preset_by_name in this scope`.

- [ ] **Step 3: Write the implementation**

Add to `src/os/editor.rs`, after `expand` and before `mod tests`:

```rust
/// A named editor invocation recipe, modelled on lazygit's editor presets.
pub struct Preset {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    /// Template used when no line number is known.
    pub edit: &'static str,
    /// Template used when a line number is known.
    pub edit_at_line: &'static str,
    /// True when the editor needs the real terminal, so the TUI must be
    /// suspended and lazygitrs must block until the editor exits.
    pub suspend: bool,
}

pub static PRESETS: &[Preset] = &[
    Preset {
        name: "vi",
        aliases: &[],
        edit: "vi -- {{filename}}",
        edit_at_line: "vi +{{line}} -- {{filename}}",
        suspend: true,
    },
    Preset {
        name: "vim",
        aliases: &[],
        edit: "vim -- {{filename}}",
        edit_at_line: "vim +{{line}} -- {{filename}}",
        suspend: true,
    },
    Preset {
        name: "nvim",
        aliases: &[],
        edit: "nvim -- {{filename}}",
        edit_at_line: "nvim +{{line}} -- {{filename}}",
        suspend: true,
    },
    Preset {
        name: "lvim",
        aliases: &[],
        edit: "lvim -- {{filename}}",
        edit_at_line: "lvim +{{line}} -- {{filename}}",
        suspend: true,
    },
    Preset {
        name: "nvim-remote",
        aliases: &["nvr"],
        edit: "nvr -- {{filename}}",
        edit_at_line: "nvr +{{line}} -- {{filename}}",
        suspend: false,
    },
    Preset {
        name: "helix",
        aliases: &["hx"],
        edit: "hx -- {{filename}}",
        edit_at_line: "hx -- {{filename}}:{{line}}",
        suspend: true,
    },
    Preset {
        name: "kakoune",
        aliases: &["kak"],
        edit: "kak -- {{filename}}",
        edit_at_line: "kak +{{line}} -- {{filename}}",
        suspend: true,
    },
    Preset {
        name: "nano",
        aliases: &[],
        edit: "nano -- {{filename}}",
        edit_at_line: "nano +{{line}} -- {{filename}}",
        suspend: true,
    },
    Preset {
        name: "micro",
        aliases: &[],
        edit: "micro {{filename}}",
        edit_at_line: "micro {{filename}}:{{line}}",
        suspend: true,
    },
    Preset {
        name: "emacs",
        aliases: &[],
        edit: "emacs --no-window-system -- {{filename}}",
        edit_at_line: "emacs --no-window-system +{{line}} -- {{filename}}",
        suspend: true,
    },
    Preset {
        name: "vscode",
        aliases: &["code"],
        edit: "code --reuse-window -- {{filename}}",
        edit_at_line: "code --reuse-window --goto -- {{filename}}:{{line}}:{{column}}",
        suspend: false,
    },
    Preset {
        name: "vscodium",
        aliases: &["codium"],
        edit: "codium --reuse-window -- {{filename}}",
        edit_at_line: "codium --reuse-window --goto -- {{filename}}:{{line}}:{{column}}",
        suspend: false,
    },
    Preset {
        name: "sublime",
        aliases: &["subl"],
        edit: "subl -- {{filename}}",
        edit_at_line: "subl -- {{filename}}:{{line}}",
        suspend: false,
    },
    Preset {
        name: "zed",
        aliases: &[],
        edit: "zed -- {{filename}}",
        edit_at_line: "zed -- {{filename}}:{{line}}:{{column}}",
        suspend: false,
    },
    Preset {
        name: "bbedit",
        aliases: &[],
        edit: "bbedit {{filename}}",
        edit_at_line: "bbedit +{{line}} {{filename}}",
        suspend: false,
    },
    Preset {
        name: "xcode",
        aliases: &["xed"],
        edit: "xed -- {{filename}}",
        edit_at_line: "xed --line {{line}} -- {{filename}}",
        suspend: false,
    },
    Preset {
        name: "acme",
        aliases: &[],
        edit: "acme {{filename}}",
        edit_at_line: "acme {{filename}}:{{line}}",
        suspend: false,
    },
    Preset {
        name: "notepadpp",
        aliases: &["notepad++"],
        edit: "notepad++ {{filename}}",
        edit_at_line: "notepad++ -n{{line}} {{filename}}",
        suspend: false,
    },
];

/// Values that are technically executable but are not editors. `$GIT_EDITOR` is
/// frequently set to `true` to suppress git's interactive editor; treating it as
/// an editor would make "open in editor" exit 0 having done nothing.
const NON_EDITORS: &[&str] = &["true", "false", ":"];

/// Look up a preset by its configured name or one of its aliases.
pub fn preset_by_name(name: &str) -> Option<&'static Preset> {
    let name = name.trim().to_ascii_lowercase();
    if name.is_empty() {
        return None;
    }
    PRESETS
        .iter()
        .find(|p| p.name == name || p.aliases.iter().any(|a| *a == name))
}

/// Return the trimmed invocation if it looks like a real editor, else `None`.
/// The whole value is preserved (not just the binary) so `EDITOR="nvim -u NONE"`
/// keeps its arguments.
pub fn usable_editor_value(value: &str) -> Option<&str> {
    let value = value.trim();
    let first = value.split_whitespace().next()?;
    if NON_EDITORS.contains(&first) {
        return None;
    }
    let stem = std::path::Path::new(first)
        .file_stem()
        .and_then(|s| s.to_str());
    if stem.is_some_and(|s| NON_EDITORS.contains(&s)) {
        return None;
    }
    Some(value)
}

/// Match a raw `$EDITOR`-style value to a preset. Pure: takes the value rather
/// than reading the environment, so it is directly testable.
pub fn preset_for_editor_string(value: &str) -> Option<&'static Preset> {
    let value = usable_editor_value(value)?;
    let first = value.split_whitespace().next()?;
    let stem = std::path::Path::new(first).file_stem()?.to_str()?;
    preset_by_name(stem)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test editor::`
Expected: PASS, 20 tests.

- [ ] **Step 5: Commit**

```bash
git add src/os/editor.rs
git commit -m "feat(os): add editor preset table and \$EDITOR matching"
```

---

### Task 4: `editPreset` and `suspendOnEdit` config fields

**Files:**
- Modify: `src/config/user_config.rs:218-261` (`OsConfig` struct and its `Default` impl)
- Test: the `mod tests` block added in Task 2

**Interfaces:**
- Consumes: nothing.
- Produces: `OsConfig::edit_preset: String` (YAML `editPreset`, default `""`) and `OsConfig::suspend_on_edit: Option<bool>` (YAML `suspendOnEdit`, default `None`). Task 5 reads both.

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `src/config/user_config.rs`:

```rust
    #[test]
    fn os_config_defaults_leave_the_editor_unconfigured() {
        let os = OsConfig::default();
        assert_eq!(os.edit_preset, "");
        assert_eq!(os.suspend_on_edit, None);
    }

    #[test]
    fn os_config_reads_lazygit_editor_keys() {
        let yaml = "editPreset: nvim\nsuspendOnEdit: false\n";
        let os: OsConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(os.edit_preset, "nvim");
        assert_eq!(os.suspend_on_edit, Some(false));
    }

    #[test]
    fn os_config_tolerates_a_config_without_editor_keys() {
        let yaml = "copyToClipboardCmd: pbcopy\n";
        let os: OsConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(os.edit_preset, "");
        assert_eq!(os.suspend_on_edit, None);
        assert_eq!(os.copy_to_clipboard_cmd, "pbcopy");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test user_config::`
Expected: FAIL to compile — `no field edit_preset on type OsConfig`.

- [ ] **Step 3: Add the fields**

In `src/config/user_config.rs`, inside `pub struct OsConfig` (after `copy_to_clipboard_cmd`, currently line 239):

```rust
    /// Named editor preset, e.g. `"nvim"` or `"vscode"`. Empty means autodetect
    /// from `$VISUAL` / `$EDITOR` / `git config core.editor`.
    #[serde(rename = "editPreset")]
    pub edit_preset: String,
    /// Force terminal handover on or off, overriding the preset's own value.
    #[serde(rename = "suspendOnEdit")]
    pub suspend_on_edit: Option<bool>,
```

And in `impl Default for OsConfig` (after `copy_to_clipboard_cmd`, currently line 258):

```rust
            edit_preset: String::new(),
            suspend_on_edit: None,
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test user_config:: && cargo build`
Expected: 6 tests PASS, build succeeds.

- [ ] **Step 5: Commit**

```bash
git add src/config/user_config.rs
git commit -m "feat(config): add os.editPreset and os.suspendOnEdit"
```

---

### Task 5: Editor resolution with precedence

**Files:**
- Modify: `src/os/editor.rs` (add above `mod tests`; extend the test module)

**Interfaces:**
- Consumes: `Preset`, `preset_by_name`, `usable_editor_value`, `preset_for_editor_string` (Task 3); `OsConfig` with `edit_preset` / `suspend_on_edit` (Task 4).
- Produces:
  - `pub struct EditorCmd { pub template: String, pub suspend: bool }`
  - `pub fn resolve_with_candidates(os: &OsConfig, line: Option<usize>, candidates: &[String]) -> Option<EditorCmd>` — pure, testable
  - `pub fn resolve(os: &OsConfig, line: Option<usize>) -> Option<EditorCmd>` — reads the environment via a memoised static

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `src/os/editor.rs`:

`OsConfig` is already in scope via the test module's `use super::*` — do **not** add another
`use crate::config::user_config::OsConfig;` inside `mod tests`, or the build fails with
"defined multiple times".

```rust
    fn os_with_preset(preset: &str) -> OsConfig {
        OsConfig {
            edit_preset: preset.to_string(),
            ..OsConfig::default()
        }
    }

    #[test]
    fn explicit_edit_at_line_beats_everything() {
        let os = OsConfig {
            edit: "myed {{filename}}".into(),
            edit_at_line: "myed +{{line}} {{filename}}".into(),
            edit_preset: "vscode".into(),
            ..OsConfig::default()
        };
        let cmd = resolve_with_candidates(&os, Some(9), &["nvim".to_string()]).unwrap();
        assert_eq!(cmd.template, "myed +{{line}} {{filename}}");
        // No preset matched, so an explicit template defaults to a terminal editor.
        assert!(cmd.suspend);
    }

    #[test]
    fn explicit_edit_is_used_when_no_line_is_known() {
        let os = OsConfig {
            edit: "myed {{filename}}".into(),
            edit_at_line: "myed +{{line}} {{filename}}".into(),
            ..OsConfig::default()
        };
        let cmd = resolve_with_candidates(&os, None, &[]).unwrap();
        assert_eq!(cmd.template, "myed {{filename}}");
    }

    #[test]
    fn preset_beats_a_detected_editor() {
        let os = os_with_preset("vscode");
        let cmd = resolve_with_candidates(&os, Some(4), &["nvim".to_string()]).unwrap();
        assert_eq!(
            cmd.template,
            "code --reuse-window --goto -- {{filename}}:{{line}}:{{column}}"
        );
        assert!(!cmd.suspend);
    }

    #[test]
    fn preset_picks_the_plain_template_without_a_line() {
        let os = os_with_preset("nvim");
        let cmd = resolve_with_candidates(&os, None, &[]).unwrap();
        assert_eq!(cmd.template, "nvim -- {{filename}}");
    }

    #[test]
    fn a_detected_editor_resolves_to_its_preset() {
        let os = OsConfig::default();
        let cmd = resolve_with_candidates(&os, Some(42), &["nvim".to_string()]).unwrap();
        assert_eq!(cmd.template, "nvim +{{line}} -- {{filename}}");
        assert!(cmd.suspend);
    }

    #[test]
    fn candidates_are_tried_in_order_and_no_ops_are_skipped() {
        let os = OsConfig::default();
        let cmd = resolve_with_candidates(
            &os,
            None,
            &["true".to_string(), "".to_string(), "nano".to_string()],
        )
        .unwrap();
        assert_eq!(cmd.template, "nano -- {{filename}}");
    }

    #[test]
    fn an_unknown_detected_editor_gets_a_generic_suspending_template() {
        let os = OsConfig::default();
        let cmd = resolve_with_candidates(&os, Some(7), &["my-weird-editor -x".to_string()])
            .unwrap();
        assert_eq!(cmd.template, "my-weird-editor -x {{filename}}");
        assert!(cmd.suspend);
    }

    #[test]
    fn suspend_on_edit_overrides_a_gui_preset() {
        let os = OsConfig {
            edit_preset: "vscode".into(),
            suspend_on_edit: Some(true),
            ..OsConfig::default()
        };
        assert!(resolve_with_candidates(&os, None, &[]).unwrap().suspend);
    }

    #[test]
    fn suspend_on_edit_overrides_a_terminal_preset() {
        let os = OsConfig {
            edit_preset: "nvim".into(),
            suspend_on_edit: Some(false),
            ..OsConfig::default()
        };
        assert!(!resolve_with_candidates(&os, None, &[]).unwrap().suspend);
    }

    #[test]
    fn nothing_resolves_when_there_is_no_config_and_no_candidate() {
        let os = OsConfig::default();
        assert!(resolve_with_candidates(&os, None, &[]).is_none());
        assert!(resolve_with_candidates(&os, None, &["true".to_string()]).is_none());
    }

    #[test]
    fn an_unknown_preset_name_falls_through_to_detection() {
        let os = os_with_preset("no-such-preset");
        let cmd = resolve_with_candidates(&os, None, &["vim".to_string()]).unwrap();
        assert_eq!(cmd.template, "vim -- {{filename}}");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test editor::`
Expected: FAIL to compile — `cannot find function resolve_with_candidates in this scope`.

- [ ] **Step 3: Write the implementation**

Add to `src/os/editor.rs`, after `preset_for_editor_string` and before `mod tests`:

```rust
use once_cell::sync::Lazy;

use crate::config::user_config::OsConfig;

/// A resolved editor invocation: a template still holding `{{...}}`
/// placeholders, plus whether running it requires the real terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorCmd {
    pub template: String,
    pub suspend: bool,
}

/// Pure core of editor resolution. `candidates` are raw `$VISUAL` / `$EDITOR` /
/// `core.editor` values in priority order, already collected by the caller.
///
/// Precedence, first match wins:
/// 1. `os.editAtLine` (when a line is known) or `os.edit` — explicit config
/// 2. `os.editPreset`
/// 3. the first usable candidate, matched to a preset
/// 4. the first usable candidate, as a generic terminal-editor template
pub fn resolve_with_candidates(
    os: &OsConfig,
    line: Option<usize>,
    candidates: &[String],
) -> Option<EditorCmd> {
    let explicit = if line.is_some() && !os.edit_at_line.is_empty() {
        Some(os.edit_at_line.clone())
    } else if !os.edit.is_empty() {
        Some(os.edit.clone())
    } else {
        None
    };
    if let Some(template) = explicit {
        // No preset to consult, so assume a terminal editor: that is both the
        // common case and the safe one (a detached GUI editor that we wait for
        // merely delays the redraw, while a terminal editor we do not wait for
        // is unusable).
        return Some(EditorCmd {
            template,
            suspend: os.suspend_on_edit.unwrap_or(true),
        });
    }

    if let Some(preset) = preset_by_name(&os.edit_preset) {
        return Some(from_preset(preset, line, os));
    }

    for candidate in candidates {
        let Some(value) = usable_editor_value(candidate) else {
            continue;
        };
        if let Some(preset) = preset_for_editor_string(value) {
            return Some(from_preset(preset, line, os));
        }
        return Some(EditorCmd {
            template: format!("{value} {{{{filename}}}}"),
            suspend: os.suspend_on_edit.unwrap_or(true),
        });
    }

    None
}

fn from_preset(preset: &Preset, line: Option<usize>, os: &OsConfig) -> EditorCmd {
    let template = if line.is_some() {
        preset.edit_at_line
    } else {
        preset.edit
    };
    EditorCmd {
        template: template.to_string(),
        suspend: os.suspend_on_edit.unwrap_or(preset.suspend),
    }
}

/// Editor candidates from the environment, in priority order.
///
/// `$GIT_EDITOR` is deliberately absent: it is git-commit-specific and is
/// commonly set to `true`, which would silently no-op every edit action.
///
/// Memoised so the `git config` subprocess runs at most once per session.
static ENV_CANDIDATES: Lazy<Vec<String>> = Lazy::new(|| {
    let mut out = Vec::new();
    for var in ["VISUAL", "EDITOR"] {
        if let Ok(value) = std::env::var(var) {
            out.push(value);
        }
    }
    if let Some(value) = git_core_editor() {
        out.push(value);
    }
    out
});

/// Read `git config --get core.editor`. Runs in the process working directory,
/// so a repository-local value is picked up when lazygitrs was launched inside
/// the repository; global and system values are found regardless.
fn git_core_editor() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["config", "--get", "core.editor"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() { None } else { Some(value) }
}

/// Resolve the editor command for an edit action, consulting the environment.
pub fn resolve(os: &OsConfig, line: Option<usize>) -> Option<EditorCmd> {
    resolve_with_candidates(os, line, &ENV_CANDIDATES)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test editor::`
Expected: PASS, 31 tests.

- [ ] **Step 5: Commit**

```bash
git add src/os/editor.rs
git commit -m "feat(os): resolve editor command from config, preset, or \$EDITOR"
```

---

### Task 6: Terminal handoff and process launching

Lands `run_with_terminal_suspended` — the helper `docs/superpowers/specs/2026-07-11-tuicr-review-integration-design.md:118` already depends on.

**Files:**
- Create: `src/gui/interactive.rs`
- Modify: `src/gui/mod.rs:1-8` (module list), `src/gui/mod.rs:7718-7774` (`setup_terminal` / `restore_terminal`)
- Test: inline `#[cfg(test)] mod tests` in `src/gui/interactive.rs`

**Interfaces:**
- Consumes: `crate::os::editor::{expand, resolve, EditorCmd}` (Tasks 1, 5); `OsConfig::run_template` (Task 2); `super::{Term, restore_terminal, enter_terminal_modes}`.
- Produces:
  - `pub struct EditRequest { pub path: String, pub line: Option<usize>, pub column: usize }` with `pub fn at(path: String, line: Option<usize>) -> Self` (column defaults to `1`)
  - `pub enum Interactive { Edit(EditRequest) }`
  - `pub fn run_with_terminal_suspended<F, R>(terminal: &mut Term, keyboard_enhanced: bool, f: F) -> Result<R> where F: FnOnce() -> R`
  - `pub enum EditError { Editor(anyhow::Error), Terminal(anyhow::Error) }`
  - `pub fn run_edit_request(terminal: &mut Term, keyboard_enhanced: bool, os: &OsConfig, req: EditRequest) -> Result<(), EditError>`

**Note on the helper signature.** The tuicr spec wrote `F: FnOnce() -> Result<R>` returning
`Result<R>`, which collapses two different failures into one error: "the callback failed" (show a
popup, keep going) and "the terminal could not be rebuilt" (fatal). Taking `F: FnOnce() -> R`
instead keeps them separate — the outer `Result` is terminal-level, and `R` carries the callback's
own outcome. Call syntax is identical, so the tuicr call site in that spec
(`run_with_terminal_suspended(term, kbd, || tuicr::run_review(opts))`) still compiles unchanged;
it just matches on `Result<Result<()>>`.

- [ ] **Step 1: Split the terminal-mode setup out of `setup_terminal`**

In `src/gui/mod.rs`, replace `fn setup_terminal` (currently lines 7718-7743) with the pair below, and change `fn restore_terminal` (line 7745) to `pub(crate) fn restore_terminal`. The escape-sequence order is unchanged from the original.

```rust
/// Enable raw mode and enter the alternate screen with mouse, focus, and paste
/// reporting. When `keyboard_enhanced` is `None` the terminal is probed and the
/// result returned; when `Some(v)` a previously probed value is reused, so a
/// re-entry after suspension cannot disagree with the original setup.
pub(crate) fn enter_terminal_modes(keyboard_enhanced: Option<bool>) -> Result<bool> {
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        crossterm::event::EnableMouseCapture,
        crossterm::event::EnableFocusChange,
        crossterm::event::EnableBracketedPaste,
        cursor::Hide
    )?;
    let enhanced = match keyboard_enhanced {
        Some(value) => value,
        None => crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false),
    };
    if enhanced {
        execute!(
            stdout,
            crossterm::event::PushKeyboardEnhancementFlags(
                crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | crossterm::event::KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                    | crossterm::event::KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
            )
        )?;
    }
    Ok(enhanced)
}

fn setup_terminal() -> Result<(Term, bool)> {
    let keyboard_enhanced = enter_terminal_modes(None)?;
    let backend = CrosstermBackend::new(io::stdout());
    let terminal = Terminal::new(backend)?;
    Ok((terminal, keyboard_enhanced))
}
```

- [ ] **Step 2: Register the new module**

In `src/gui/mod.rs`, the module list (lines 1-8) becomes:

```rust
pub mod context;
pub mod controller;
pub mod interactive;
pub mod layout;
pub mod modes;
pub mod popup;
pub mod presentation;
pub mod scroll;
pub mod views;
```

- [ ] **Step 3: Write the failing tests**

Create `src/gui/interactive.rs` containing *only* the test module for now:

`EditorCmd` and `OsConfig` arrive via `use super::*` (the parent module imports both) — do not
re-import them inside `mod tests`.

```rust
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
        assert!(err.contains('3'), "exit code missing from: {err}");
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
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test interactive::`
Expected: FAIL to compile — `cannot find type EditRequest in this scope`.

- [ ] **Step 5: Write the implementation**

Prepend to `src/gui/interactive.rs`, above the test module:

```rust
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

/// Hand the terminal to `f`, then restore it — on every exit path, including a
/// panic inside `f`.
///
/// The returned `Result` reports only whether the *terminal* survived. `f`'s own
/// outcome comes back as `R`, so a caller can tell "the editor failed" (worth a
/// popup) from "the terminal is gone" (fatal).
pub fn run_with_terminal_suspended<F, R>(
    terminal: &mut Term,
    keyboard_enhanced: bool,
    f: F,
) -> Result<R>
where
    F: FnOnce() -> R,
{
    restore_terminal(terminal, keyboard_enhanced)?;

    let outcome = panic::catch_unwind(AssertUnwindSafe(f));

    // Rebuild before inspecting the outcome: a panic or an error must not be
    // allowed to leave the terminal in raw mode. `clear()` is required because
    // ratatui diffs against its own buffer and would otherwise keep cells the
    // editor has since overwritten.
    let rebuilt = (|| -> Result<()> {
        enter_terminal_modes(Some(keyboard_enhanced))?;
        terminal.clear()?;
        Ok(())
    })();

    match outcome {
        Ok(value) => {
            rebuilt?;
            Ok(value)
        }
        Err(payload) => panic::resume_unwind(payload),
    }
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
            let editor_outcome =
                run_with_terminal_suspended(terminal, keyboard_enhanced, || {
                    run_editor_blocking(&cmd, &req)
                })
                .map_err(EditError::Terminal)?;
            editor_outcome.map_err(EditError::Editor)
        }
        Some(cmd) => run_editor_detached(&cmd, &req).map_err(EditError::Editor),
        None => open_with_default_program(os, &req.path).map_err(EditError::Editor),
    }
}
```

- [ ] **Step 6: Run the tests and build**

Run: `cargo test interactive:: && cargo build`
Expected: 6 tests PASS, build clean. `run_edit_request` has no caller yet, but it is `pub` in a
`pub mod`, so no dead-code warning is expected.

- [ ] **Step 7: Commit**

```bash
git add src/gui/interactive.rs src/gui/mod.rs
git commit -m "feat(gui): add terminal-suspend helper and edit request execution"
```

---

### Task 7: Drain the request in `main_loop`

**Files:**
- Modify: `src/gui/mod.rs:284-300` (`Gui` struct), `src/gui/mod.rs:550-560` (initialiser), `src/gui/mod.rs:656` (`main_loop` call), `src/gui/mod.rs:662` (`main_loop` signature), `src/gui/mod.rs:1011` (drain point)

**Interfaces:**
- Consumes: `interactive::{Interactive, run_edit_request}` (Task 6).
- Produces: `Gui::pending_interactive: Option<interactive::Interactive>`. Tasks 8 and 9 set it.

- [ ] **Step 1: Add the field**

In `src/gui/mod.rs`, inside `pub struct Gui` after `pub needs_diff_refresh: bool,` (line 297):

```rust
    /// An action that needs the real terminal, queued by a key handler and
    /// executed by `main_loop`, which owns the `Terminal`.
    pub pending_interactive: Option<interactive::Interactive>,
```

And in the initialiser, after `needs_diff_refresh: true,` (line 557):

```rust
            pending_interactive: None,
```

- [ ] **Step 2: Thread `keyboard_enhanced` into `main_loop`**

In `src/gui/mod.rs`, line 656 becomes:

```rust
        let result = self.main_loop(&mut terminal, keyboard_enhanced);
```

And line 662 becomes:

```rust
    fn main_loop(&mut self, terminal: &mut Term, keyboard_enhanced: bool) -> Result<()> {
```

- [ ] **Step 3: Insert the drain**

In `src/gui/mod.rs`, between the end of the event-handling block (`}` closing `if event::poll(...)`, line 1010) and the `if self.should_quit {` check (line 1012), insert:

```rust
            // Run any action that needs the real terminal. This is the only
            // place that hands the terminal over, so key handlers stay pure.
            if let Some(action) = self.pending_interactive.take() {
                match action {
                    interactive::Interactive::Edit(req) => {
                        let os = self.config.user_config.os.clone();
                        match interactive::run_edit_request(
                            terminal,
                            keyboard_enhanced,
                            &os,
                            req,
                        ) {
                            Ok(()) => {}
                            Err(interactive::EditError::Editor(err)) => {
                                self.show_error("Editor failed", err)
                            }
                            // The terminal could not be rebuilt, so nothing can
                            // be drawn. Bail out and let run()'s
                            // restore_terminal do the final cleanup.
                            Err(interactive::EditError::Terminal(err)) => return Err(err),
                        }
                    }
                }
                // The file may have changed on disk; refresh() also sets
                // needs_diff_refresh, so the open diff reloads too.
                self.needs_refresh = true;
            }

```

`os` is cloned because `run_edit_request` needs `&OsConfig` while `self` is mutably borrowed for `show_error`. `OsConfig` derives `Clone` (`src/config/user_config.rs:218`) and is a handful of strings.

- [ ] **Step 4: Build and verify no warnings**

Run: `cargo build 2>&1 | grep -E "^error" || echo "no errors"`
Expected: `no errors`.

- [ ] **Step 5: Run the full test suite**

Run: `cargo test`
Expected: PASS, all existing tests plus the 36 added so far.

- [ ] **Step 6: Commit**

```bash
git add src/gui/mod.rs
git commit -m "feat(gui): execute pending interactive actions in the main loop"
```

---

### Task 8: Migrate the five existing edit call sites

Each site keeps its existing path/line logic and stops running templates itself.

**Files:**
- Modify: `src/gui/controller/files.rs:492-542` (`open_in_editor`)
- Modify: `src/gui/mod.rs:2900-2949` (`open_diff_file_in_editor`)
- Modify: `src/gui/mod.rs:2550-2601` (diff text selection `e`)
- Modify: `src/gui/controller/diff_mode.rs:471-536` (diff-mode text selection `e`)
- Modify: `src/gui/mod.rs:5045-5058` (`open_conflict_file_in_editor`)
- Modify: `src/config/user_config.rs` (delete `run_template_at_line`, now unused)

**Interfaces:**
- Consumes: `crate::gui::interactive::{EditRequest, Interactive}` (Task 6); `Gui::pending_interactive` (Task 7).
- Produces: no new interfaces.

- [ ] **Step 1: Migrate `files.rs::open_in_editor`**

Replace the body of `open_in_editor` (`src/gui/controller/files.rs:492-542`) with:

```rust
fn open_in_editor(gui: &mut Gui) -> Result<()> {
    let Some(file_idx) = gui.selected_file_index() else {
        return Ok(());
    };
    let model = gui.model.lock().unwrap();
    let Some(file) = model.files.get(file_idx) else {
        return Ok(());
    };
    let rel_path = file.current_path().to_string();
    drop(model);

    let abs_path_buf = gui.git.repo_path().join(&rel_path);
    if !abs_path_buf.exists() {
        anyhow::bail!("file does not exist: {rel_path}");
    }
    let abs_path = abs_path_buf.to_string_lossy().to_string();

    // Jump to the first changed hunk if the diff for this file is loaded.
    let first_hunk_line = if gui.diff_view.filename == rel_path {
        gui.diff_view.hunk_starts.first().and_then(|&idx| {
            gui.diff_view
                .file_line_number(idx, DiffPanel::New)
                .or_else(|| gui.diff_view.file_line_number(idx, DiffPanel::Old))
        })
    } else {
        None
    };

    gui.pending_interactive = Some(Interactive::Edit(EditRequest::at(
        abs_path,
        first_hunk_line,
    )));
    Ok(())
}
```

Add to the imports at the top of `src/gui/controller/files.rs`:

```rust
use crate::gui::interactive::{EditRequest, Interactive};
```

- [ ] **Step 2: Migrate `Gui::open_diff_file_in_editor`**

Replace `src/gui/mod.rs:2900-2949` with:

```rust
    fn open_diff_file_in_editor(&mut self) {
        let rel_path = self.diff_view.filename.clone();
        if rel_path.is_empty() {
            return;
        }
        let abs_path_buf = self.git.repo_path().join(&rel_path);
        if !abs_path_buf.exists() {
            return;
        }
        let abs_path = abs_path_buf.to_string_lossy().to_string();

        // Pick the hunk currently at the top of the viewport (after `{`/`}`
        // navigation, scroll_offset sits on a hunk start). Fall back to the
        // most recent hunk before the viewport, then the first hunk.
        let active_hunk_idx = self
            .diff_view
            .hunk_starts
            .iter()
            .rev()
            .find(|&&h| h <= self.diff_view.scroll_offset)
            .copied()
            .or_else(|| self.diff_view.hunk_starts.first().copied());

        let active_hunk_line = active_hunk_idx.and_then(|idx| {
            self.diff_view
                .file_line_number(idx, DiffPanel::New)
                .or_else(|| self.diff_view.file_line_number(idx, DiffPanel::Old))
        });

        self.pending_interactive = Some(interactive::Interactive::Edit(
            interactive::EditRequest::at(abs_path, active_hunk_line),
        ));
    }
```

- [ ] **Step 3: Migrate the diff text-selection site in `gui/mod.rs`**

In `src/gui/mod.rs`, replace lines 2578-2600 — everything from `self.diff_view.selection = None;` up to and including the closing brace before `return Ok(());` — with:

```rust
                    self.diff_view.selection = None;
                    let abs_path = self.git.repo_path().join(&filename);
                    if !filename.is_empty() && abs_path.exists() {
                        let line = line
                            .or_else(|| self.diff_view.file_line_number(line_idx, line_panel));
                        self.pending_interactive =
                            Some(interactive::Interactive::Edit(interactive::EditRequest {
                                path: abs_path.to_string_lossy().to_string(),
                                line,
                                column,
                            }));
                    }
                    return Ok(());
```

- [ ] **Step 4: Migrate the diff-mode text-selection site**

In `src/gui/controller/diff_mode.rs`, replace lines 514-534 — from `gui.diff_view.selection = None;` up to the closing brace before `return Ok(());` — with:

```rust
                gui.diff_view.selection = None;
                let abs_path = gui.git.repo_path().join(&filename);
                if !filename.is_empty() && abs_path.exists() {
                    let line =
                        line.or_else(|| gui.diff_view.file_line_number(line_idx, line_panel));
                    gui.pending_interactive = Some(Interactive::Edit(EditRequest {
                        path: abs_path.to_string_lossy().to_string(),
                        line,
                        column,
                    }));
                }
                return Ok(());
```

Add to the imports at the top of `src/gui/controller/diff_mode.rs`:

```rust
use crate::gui::interactive::{EditRequest, Interactive};
```

- [ ] **Step 5: Migrate `Gui::open_conflict_file_in_editor`**

Replace `src/gui/mod.rs:5045-5058` with:

```rust
    fn open_conflict_file_in_editor(&mut self, path: &str) -> Result<()> {
        let abs_path_buf = self.git.repo_path().join(path);
        if !abs_path_buf.exists() {
            anyhow::bail!("file does not exist: {path}");
        }
        self.pending_interactive = Some(interactive::Interactive::Edit(
            interactive::EditRequest::at(abs_path_buf.to_string_lossy().to_string(), None),
        ));
        Ok(())
    }
```

`execute_menu_action` already replaced the popup with `PopupState::None` before invoking the action (`src/gui/mod.rs:1344`), so the conflict menu is closed by the time the drain runs.

- [ ] **Step 6: Delete the now-unused `run_template_at_line`**

Remove the whole `run_template_at_line` method from `src/config/user_config.rs`, and remove the one
test that references it from that file's `mod tests`:
`run_template_at_line_rejects_an_empty_template`. Its remaining callers were all migrated in
Steps 1-5, so leaving it would be dead code.

- [ ] **Step 7: Build and check for warnings**

Run: `cargo build 2>&1 | grep -E "^error" || echo "no errors"`
Expected: `no errors`. If an unused-import warning appears for `DiffPanel` in `files.rs`, that means Step 1's hunk-line logic was dropped — restore it.

- [ ] **Step 8: Run the full test suite**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 9: Manually verify all five migrated contexts**

```bash
cargo build && ./target/debug/lazygitrs
```

With `$EDITOR=nvim` and no `os:` block in the config, check each one. In every case nvim must take
over the terminal, and on `:q` lazygitrs must redraw with no stale cells.

1. **Files panel** — select a modified file, press `e`. Expected: opens at the first changed hunk.
   Then edit, save, `:q`. Expected: the file list and the open diff both reflect the change with no
   manual refresh.
2. **Diff panel focused** — `<enter>` into the diff, navigate with `{`/`}`, press `e`. Expected:
   opens at the hunk currently at the top of the viewport.
3. **Diff text selection** — drag-select text in the diff, press `e`. Expected: opens at the
   selected line *and* column.
4. **Diff mode** — enter diff mode, select text in the diff, press `e`. Expected: same as 3.
5. **Conflict resolution** — with a conflicted file selected in Files, open the conflict menu and
   choose "Open in editor" (`e`). Expected: the menu closes and nvim opens the conflicted file.

- [ ] **Step 10: Commit**

```bash
git add src/gui/mod.rs src/gui/controller/files.rs src/gui/controller/diff_mode.rs src/config/user_config.rs
git commit -m "fix(gui): open files in a terminal editor by suspending the TUI"
```

---

### Task 9: Bind `e` in the Commit Files panel

`commit_files::handle_key` serves `CommitFiles`, `StashFiles`, and `BranchCommitFiles` (`src/gui/mod.rs:2504`), so one binding covers all three. A historical blob cannot be edited in place, so this opens the **working-tree** version and errors if the path is gone.

**Files:**
- Modify: `src/gui/controller/commit_files.rs:12-71` (`handle_key`), plus a new function
- Modify: `src/gui/mod.rs:4574` (`BranchCommits | BranchCommitFiles` help), `src/gui/mod.rs:4688` (`CommitFiles` help), `src/gui/mod.rs:4767` (`StashFiles` help)

**Interfaces:**
- Consumes: `crate::gui::interactive::{EditRequest, Interactive}` (Task 6); `Gui::pending_interactive` (Task 7).
- Produces: no new interfaces.

- [ ] **Step 1: Add the key branch**

In `src/gui/controller/commit_files.rs`, insert before the `// Copy to clipboard` block (line 65):

```rust
    // Open the working-tree version of the selected file in the editor
    if matches_key(key, &keybindings.universal.edit) {
        return open_in_editor(gui);
    }

```

- [ ] **Step 2: Add the function**

Add to `src/gui/controller/commit_files.rs`, after `copy_to_clipboard_menu`:

```rust
/// Open the selected commit file in the editor. A commit's blob cannot be
/// edited in place, so this opens the working-tree copy of the same path.
fn open_in_editor(gui: &mut Gui) -> Result<()> {
    let selected = gui.context_mgr.selected_active();

    // Tree view maps a node to a file index; list view selects directly.
    let file_idx = if gui.show_commit_file_tree {
        gui.commit_file_tree_nodes
            .get(selected)
            .and_then(|n| n.file_index)
    } else {
        Some(selected)
    };
    let Some(idx) = file_idx else { return Ok(()) };

    let model = gui.model.lock().unwrap();
    let Some(file) = model.commit_files.get(idx) else {
        return Ok(());
    };
    let rel_path = file.current_path().to_string();
    drop(model);

    let abs_path_buf = gui.git.repo_path().join(&rel_path);
    if !abs_path_buf.exists() {
        anyhow::bail!("no longer in the working tree: {rel_path}");
    }

    gui.pending_interactive = Some(Interactive::Edit(EditRequest::at(
        abs_path_buf.to_string_lossy().to_string(),
        None,
    )));
    Ok(())
}
```

Add to the imports at the top of `src/gui/controller/commit_files.rs`:

```rust
use crate::gui::interactive::{EditRequest, Interactive};
```

- [ ] **Step 3: Add the help entries**

Add this `HelpEntry` to the `entries` vector of all three help sections — `src/gui/mod.rs:4574` (`ContextId::BranchCommits | ContextId::BranchCommitFiles`), `:4688` (`ContextId::CommitFiles`), and `:4767` (`ContextId::StashFiles`) — matching the existing wording at `:4446`:

```rust
                    HelpEntry {
                        key: kb.universal.edit.clone(),
                        description: "Open in editor".into(),
                    },
```

- [ ] **Step 4: Build and test**

Run: `cargo build && cargo test`
Expected: build clean, all tests PASS.

- [ ] **Step 5: Manually verify**

```bash
./target/debug/lazygitrs
```

- In Commits, press Enter to list a commit's files, select one, press `e`. Expected: nvim opens the working-tree copy.
- Select a file that was deleted after that commit and press `e`. Expected: a `"no longer in the working tree"` error popup, not an empty new file.
- Press `?` in each of Commit Files, Stash Files, and Branch Commit Files. Expected: `e  Open in editor` is listed.

- [ ] **Step 6: Audit the help dialog**

Run the repository's `audit-help` skill and fix anything it reports for the three sections touched here.

- [ ] **Step 7: Commit**

```bash
git add src/gui/controller/commit_files.rs src/gui/mod.rs
git commit -m "feat(gui): open working-tree file from the commit files panel with e"
```

---

## Final verification

- [ ] **Full suite and clean build**

```bash
cargo test && cargo build 2>&1 | grep -E "^error" || echo "no errors"
```

- [ ] **Preset override path**

Add to `~/.config/lazygitrs/config.yml`:

```yaml
os:
  editPreset: vscode
```

Press `e` in the Files panel. Expected: the editor launches detached and the TUI is never torn down. Remove the block afterwards.

- [ ] **Silent-failure guard**

Add to `~/.config/lazygitrs/config.yml`:

```yaml
os:
  edit: 'definitely-not-a-real-editor {{filename}}'
```

Press `e`. Expected: an `"editor not found (status 127)"` popup — not a no-op. Remove the block afterwards.

- [ ] **`$GIT_EDITOR` is not consulted**

With `GIT_EDITOR=true` in the environment (already the case on the target machine) and `VISUAL`/`EDITOR` unset, press `e`. Expected: the `xdg-open` fallback or the "No editor configured" popup — never a silent no-op.

```bash
env -u VISUAL -u EDITOR GIT_EDITOR=true ./target/debug/lazygitrs
```

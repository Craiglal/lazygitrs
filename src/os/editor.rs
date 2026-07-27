//! Resolves which editor command to run for an "open in editor" action, and
//! expands its command template into a shell command line.

use once_cell::sync::Lazy;

use crate::config::user_config::OsConfig;

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
        // `nvr` with no running server execs a fresh interactive nvim that
        // needs the tty, so suspending is the only safe default. When a server
        // IS already listening nvr returns immediately and the only cost is one
        // redraw of the restored TUI.
        suspend: true,
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
        // micro's `file:line` suffix is only honoured when the `parsecursor`
        // option is on, and it defaults to off — with the default config that
        // form opens a new empty buffer named "file.rs:42" instead of the file.
        // The `+LINE` flag is parsed unconditionally.
        edit_at_line: "micro +{{line}} {{filename}}",
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
        // A known line number is necessarily dropped here: we have no idea what
        // at-line syntax an unrecognised editor accepts, so the file is opened at
        // the top rather than risking a bogus argument.
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
            expand(
                "vim +{{line}} -- {{filename}}",
                "/tmp/{{line}}.rs",
                Some(5),
                1
            ),
            "vim +5 -- '/tmp/{{line}}.rs'"
        );
    }

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
        assert_eq!(
            preset_for_editor_string("nvim -u NONE").unwrap().name,
            "nvim"
        );
    }

    #[test]
    fn preset_for_editor_string_rejects_no_op_binaries() {
        // Behavioural check only. Note this test cannot detect the loss of the
        // `usable_editor_value` delegation, because none of these values is a
        // preset name either — the actual guard for the shared rejection list
        // is `usable_editor_value_rejects_no_op_binaries` below.
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
        assert_eq!(
            usable_editor_value("my-weird-editor"),
            Some("my-weird-editor")
        );
        assert!(preset_for_editor_string("my-weird-editor").is_none());
    }

    #[test]
    fn micro_jumps_to_a_line_with_the_flag_form_not_the_colon_form() {
        // The colon form depends on micro's `parsecursor` option, which is off
        // by default and would open a new empty buffer instead of the file.
        let micro = preset_by_name("micro").unwrap();
        assert_eq!(micro.edit_at_line, "micro +{{line}} {{filename}}");
        assert!(!micro.edit_at_line.contains("{{filename}}:{{line}}"));
    }

    #[test]
    fn nvim_remote_suspends_because_it_may_exec_an_interactive_nvim() {
        assert!(preset_by_name("nvr").unwrap().suspend);
    }

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
        let cmd =
            resolve_with_candidates(&os, Some(7), &["my-weird-editor -x".to_string()]).unwrap();
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

    #[test]
    fn suspend_on_edit_overrides_an_explicit_config_template() {
        // Guards the explicit branch: with suspend_on_edit left at None a
        // hardcoded `true` here would be indistinguishable from unwrap_or(true).
        let os = OsConfig {
            edit: "myed {{filename}}".into(),
            suspend_on_edit: Some(false),
            ..OsConfig::default()
        };
        let cmd = resolve_with_candidates(&os, None, &[]).unwrap();
        assert_eq!(cmd.template, "myed {{filename}}");
        assert!(!cmd.suspend);
    }

    #[test]
    fn suspend_on_edit_overrides_a_generic_detected_editor() {
        // Same guard for the generic-fallback branch.
        let os = OsConfig {
            suspend_on_edit: Some(false),
            ..OsConfig::default()
        };
        let cmd = resolve_with_candidates(&os, None, &["my-weird-editor".to_string()]).unwrap();
        assert_eq!(cmd.template, "my-weird-editor {{filename}}");
        assert!(!cmd.suspend);
    }

    #[test]
    fn the_first_usable_candidate_wins_even_when_a_later_one_matches_a_preset() {
        // $VISUAL outranks $EDITOR per POSIX convention, so an unrecognised
        // first candidate is used generically and nvim's preset is never
        // consulted. Pinned because "prefer any preset match" is an easy
        // mis-edit that would otherwise pass every test.
        let os = OsConfig::default();
        let cmd = resolve_with_candidates(
            &os,
            Some(7),
            &["my-weird-editor".to_string(), "nvim".to_string()],
        )
        .unwrap();
        assert_eq!(cmd.template, "my-weird-editor {{filename}}");
        assert!(cmd.suspend);
    }

    #[test]
    fn explicit_edit_beats_a_preset_when_a_line_is_known() {
        // edit_at_line is empty, so the no-line `edit` template is used even
        // though a line is known — explicit config still outranks the preset.
        let os = OsConfig {
            edit: "myed {{filename}}".into(),
            edit_preset: "nvim".into(),
            ..OsConfig::default()
        };
        let cmd = resolve_with_candidates(&os, Some(9), &[]).unwrap();
        assert_eq!(cmd.template, "myed {{filename}}");
    }
}

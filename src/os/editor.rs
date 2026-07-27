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
        assert_eq!(
            usable_editor_value("my-weird-editor"),
            Some("my-weird-editor")
        );
        assert!(preset_for_editor_string("my-weird-editor").is_none());
    }
}

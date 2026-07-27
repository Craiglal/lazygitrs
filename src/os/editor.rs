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
    template
        .replace("{{filename}}", &shell_quote(path))
        .replace("{{line}}", &line.unwrap_or(1).to_string())
        .replace("{{column}}", &column.to_string())
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
}

//! Light/dark appearance detection for the "System" color theme.
//!
//! For a TUI the ground truth is the *terminal's* background color, not the
//! desktop's color-scheme preference.  The two routinely disagree — a kitty
//! config that pins `background #181616` stays dark no matter what GTK
//! reports — and trusting the desktop in that case paints dark text on a dark
//! background.  So we ask the terminal itself (OSC 11) and only fall back to
//! environment hints.

use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}

static DETECTED: OnceLock<Appearance> = OnceLock::new();

/// The appearance detected at startup.
///
/// Cheap enough for the render path — it only reads the cached value, and
/// falls back to [`Appearance::Dark`] if [`detect`] was never called.
pub fn appearance() -> Appearance {
    DETECTED.get().copied().unwrap_or(Appearance::Dark)
}

/// Query the terminal for its appearance and cache the result.
///
/// Must run *before* the crossterm event stream starts polling stdin: the
/// OSC 11 reply arrives on stdin, so a concurrent event reader would swallow
/// it.  Repeat calls are a no-op and return the first result.
pub fn detect() -> Appearance {
    *DETECTED.get_or_init(|| {
        from_terminal()
            .or_else(from_colorfgbg)
            .unwrap_or(Appearance::Dark)
    })
}

/// Ask the terminal over OSC 11.  Returns `None` when the terminal doesn't
/// answer (no tty, dumb terminal, timeout).
fn from_terminal() -> Option<Appearance> {
    use terminal_colorsaurus::{QueryOptions, ThemeMode, theme_mode};

    match theme_mode(QueryOptions::default()) {
        Ok(ThemeMode::Light) => Some(Appearance::Light),
        Ok(ThemeMode::Dark) => Some(Appearance::Dark),
        Err(_) => None,
    }
}

/// `COLORFGBG` is set by terminals that never answer OSC 11 (urxvt, some
/// konsole builds).  The value is `fg;bg` or `fg;<extra>;bg` where the fields
/// are ANSI palette indices; 0-6 and 8 are the dark backgrounds.
fn from_colorfgbg() -> Option<Appearance> {
    parse_colorfgbg(&std::env::var("COLORFGBG").ok()?)
}

fn parse_colorfgbg(raw: &str) -> Option<Appearance> {
    let bg: u8 = raw.rsplit(';').next()?.trim().parse().ok()?;
    Some(if matches!(bg, 0..=6 | 8) {
        Appearance::Dark
    } else {
        Appearance::Light
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorfgbg_dark_backgrounds_are_detected() {
        for value in ["15;0", "default;0", "7;0;0", "15;6", "15;8"] {
            assert_eq!(parse_colorfgbg(value), Some(Appearance::Dark), "{value}");
        }
    }

    #[test]
    fn colorfgbg_light_backgrounds_are_detected() {
        for value in ["0;15", "0;7", "0;0;15"] {
            assert_eq!(parse_colorfgbg(value), Some(Appearance::Light), "{value}");
        }
    }

    /// A malformed value must yield no opinion, so the caller falls through to
    /// the dark default rather than guessing light.
    #[test]
    fn colorfgbg_garbage_yields_no_opinion() {
        for value in ["", "nonsense", "15;rgb:ffff", "0;999"] {
            assert_eq!(parse_colorfgbg(value), None, "{value}");
        }
    }

    /// The theme registry hard-codes these ids; a rename here silently
    /// downgrades every affected user to the dark fallback.
    #[test]
    fn builtin_theme_ids_resolve_to_distinct_themes() {
        use crate::config::theme::load_color_themes;

        let themes = load_color_themes();
        let ids: Vec<&str> = themes.iter().take(3).map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["system", "default", "default-light"]);

        // "default-light" must actually produce the light theme, not fall
        // through to the dark fallback at the end of `to_theme`.
        let light = themes[2].to_theme();
        assert_eq!(light.text_strong, crate::config::Theme::light().text_strong);
        assert_ne!(light.text_strong, crate::config::Theme::dark().text_strong);
    }
}

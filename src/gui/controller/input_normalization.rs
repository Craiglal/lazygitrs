pub fn replace_spaces_with_dashes(raw: &str) -> String {
    raw.trim().replace(' ', "-")
}

#[cfg(test)]
mod tests {
    use super::replace_spaces_with_dashes;

    #[test]
    fn trims_and_replaces_spaces_with_dashes() {
        assert_eq!(
            replace_spaces_with_dashes("feature branch"),
            "feature-branch"
        );
        assert_eq!(
            replace_spaces_with_dashes("  spaced branch  "),
            "spaced-branch"
        );
        assert_eq!(
            replace_spaces_with_dashes("feature  branch"),
            "feature--branch"
        );
        assert_eq!(
            replace_spaces_with_dashes("feature/child branch"),
            "feature/child-branch"
        );
    }
}

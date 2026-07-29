# Minimal assertion helpers for the contrib/ shell tests.
# Source this file, call the assert_* helpers, then end with test_summary.

TESTS_RUN=0
TESTS_FAILED=0

_report_pass() {
    TESTS_RUN=$((TESTS_RUN + 1))
    printf '  \033[32m✓\033[0m %s\n' "$1"
}

_report_fail() {
    TESTS_RUN=$((TESTS_RUN + 1))
    TESTS_FAILED=$((TESTS_FAILED + 1))
    printf '  \033[31m✗\033[0m %s\n' "$1"
    printf '      %s\n' "$2"
}

# assert_eq <name> <expected> <actual>
assert_eq() {
    if [ "$2" = "$3" ]; then
        _report_pass "$1"
    else
        _report_fail "$1" "expected [$2], got [$3]"
    fi
}

# assert_contains <name> <haystack> <needle>
assert_contains() {
    case "$2" in
        *"$3"*) _report_pass "$1" ;;
        *) _report_fail "$1" "expected to contain [$3], got [$2]" ;;
    esac
}

test_summary() {
    printf '\n  %d test(s), %d failure(s)\n\n' "$TESTS_RUN" "$TESTS_FAILED"
    [ "$TESTS_FAILED" -eq 0 ]
}

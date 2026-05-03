//! Integration test: end-to-end sanitization of operator-visible strings.
//!
//! Mirrors the unit tests in `src/sanitize.rs` from outside the crate so
//! the public surface exposed via `lib.rs` stays stable.

use dux::sanitize;

#[test]
fn end_to_end_log_line_has_no_escapes() {
    let evil_branch = "feat-\x1b]2;evil\x07-x";
    let cleaned = sanitize::for_terminal(evil_branch);
    assert!(!cleaned.contains('\x1b'));
    assert!(!cleaned.contains('\x07'));
    // Sanitized form preserves the readable surrounding text.
    assert!(cleaned.contains("feat-"));
    assert!(cleaned.ends_with("-x"));
}

#[test]
fn utf8_lossy_strips_escape_from_invalid_utf8() {
    // Mimics `git stderr` carrying invalid UTF-8 around an injected
    // OSC-2 (set-window-title) sequence.
    let bytes = b"git error: \xff\x1b]2;rm -rf $HOME\x07 hi";
    let out = sanitize::utf8_lossy(bytes);
    assert!(!out.contains('\x1b'));
    assert!(!out.contains('\x07'));
    assert!(out.contains("git error:"));
}

#[test]
fn truncate_caps_length_and_appends_ellipsis() {
    // 50 hex-expanded escapes would otherwise produce ~200 chars; the
    // status-line cap keeps it bounded.
    let evil = "\x1b".repeat(50);
    let out = sanitize::truncate(&evil, 16);
    assert_eq!(out.chars().count(), 16);
    assert!(out.ends_with('…'));
}

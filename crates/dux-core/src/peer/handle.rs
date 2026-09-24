//! Agent-handle and operator-text helpers the peer router needs.
//!
//! INTEGRATION: the fork kept these in `model.rs` (`AGENT_HANDLE_MAX_LEN`,
//! `normalize_agent_handle`, `is_valid_agent_handle`), `storage.rs`
//! (`next_unique_agent_handle`) and `sanitize.rs` (`amq_handle`,
//! `for_terminal`, `truncate`). The shared-workspace port owns the persisted
//! identity and will add the canonical versions; once it lands, these should
//! become re-exports of those so there is one alphabet and one length bound.

use std::collections::HashSet;

/// Upper bound on a persisted agent handle, in bytes (the alphabet is ASCII).
pub const AGENT_HANDLE_MAX_LEN: usize = 64;

/// Sanitise an AMQ/Dux peer handle the same way the AMQ wrappers do:
/// lowercase ASCII, keep `[a-z0-9_-]`, replace other characters with `-`,
/// then trim leading/trailing dashes.
pub fn amq_handle(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-' {
            out.push(ch);
        } else if ch.is_ascii_uppercase() {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    trimmed.to_string()
}

/// Normalize a newly created identity. Never used to rewrite a stored handle.
pub fn normalize_agent_handle(candidate: &str) -> String {
    amq_handle(candidate)
        .chars()
        .take(AGENT_HANDLE_MAX_LEN)
        .collect()
}

/// Derive the basename-first identity a session gets when it has no stored
/// handle: the directory's name, else the branch, else the session id.
pub fn derive_agent_handle(directory: &str, branch_name: Option<&str>, id: &str) -> String {
    let basename = std::path::Path::new(directory)
        .file_name()
        .and_then(|part| part.to_str());
    basename
        .into_iter()
        .chain(branch_name)
        .chain([id])
        .map(normalize_agent_handle)
        .find(|handle| !handle.is_empty())
        .unwrap_or_else(|| "agent".to_string())
}

/// Validate the exact persisted handle alphabet and length contract.
pub fn is_valid_agent_handle(handle: &str) -> bool {
    (1..=AGENT_HANDLE_MAX_LEN).contains(&handle.len())
        && handle
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(&byte))
}

/// The first of `base`, `base-2`, `base-3`, ... that `used` does not hold,
/// truncating the prefix so the suffixed handle stays within the length bound.
pub fn next_unique_agent_handle(base: &str, used: &HashSet<String>) -> String {
    if !used.contains(base) {
        return base.to_string();
    }
    for ordinal in 2u64.. {
        let candidate = suffixed_handle(base, ordinal);
        if !used.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!("unbounded handle suffix search")
}

/// `base` with `-<ordinal>` appended, the prefix cut so the result fits
/// [`AGENT_HANDLE_MAX_LEN`].
pub(crate) fn suffixed_handle(base: &str, ordinal: u64) -> String {
    let suffix = format!("-{ordinal}");
    let prefix_len = AGENT_HANDLE_MAX_LEN.saturating_sub(suffix.len());
    let prefix = base.chars().take(prefix_len).collect::<String>();
    format!("{prefix}{suffix}")
}

/// Strip control bytes and ESC from operator-visible text; keep printable
/// characters, `\t` and `\n`. Stripped characters render as `\xNN` so nothing
/// is lost silently. Paths and handles in error messages pass through this so
/// a hostile directory name cannot inject terminal escape sequences.
pub fn for_terminal(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' | '\t' => out.push(c),
            c if c.is_control() => {
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

/// Lossy UTF-8 decode of command output, sanitised for the terminal.
pub fn utf8_lossy(bytes: &[u8]) -> String {
    for_terminal(&String::from_utf8_lossy(bytes))
}

/// Sanitise, then cap at `max_chars` characters with an ellipsis.
pub fn truncate(s: &str, max_chars: usize) -> String {
    let cleaned = for_terminal(s);
    if cleaned.chars().count() <= max_chars {
        cleaned
    } else {
        cleaned
            .chars()
            .take(max_chars.saturating_sub(1))
            .chain(std::iter::once('\u{2026}'))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amq_handle_lowercases_replaces_and_trims() {
        assert_eq!(amq_handle("Feature Login!"), "feature-login");
        assert_eq!(amq_handle("--a_b-9--"), "a_b-9");
        assert_eq!(amq_handle("!!!"), "");
    }

    #[test]
    fn amq_handle_matches_wrapper_rules() {
        assert_eq!(amq_handle("Feature/Login"), "feature-login");
        assert_eq!(amq_handle("--Already_OK--"), "already_ok");
        assert_eq!(amq_handle("!!!"), "");
    }

    #[test]
    fn derived_handle_prefers_basename_then_branch_then_id() {
        assert_eq!(
            derive_agent_handle("/w/Feature Login", Some("x"), "id"),
            "feature-login"
        );
        assert_eq!(
            derive_agent_handle("/w/!!!", Some("Branch"), "id"),
            "branch"
        );
        assert_eq!(derive_agent_handle("/", None, "S1"), "s1");
        assert_eq!(derive_agent_handle("/", None, "!!"), "agent");
    }

    #[test]
    fn unique_handle_suffixes_within_bound() {
        let base = "a".repeat(AGENT_HANDLE_MAX_LEN);
        let used = HashSet::from([base.clone()]);
        let next = next_unique_agent_handle(&base, &used);
        assert_eq!(next.len(), AGENT_HANDLE_MAX_LEN);
        assert!(next.ends_with("-2"));
        assert!(is_valid_agent_handle(&next));
        assert!(!is_valid_agent_handle(""));
        assert!(!is_valid_agent_handle("Upper"));
    }

    #[test]
    fn for_terminal_escapes_control_bytes() {
        assert_eq!(for_terminal("a\u{1b}]8;;x\u{7}b\n"), "a\\x1b]8;;x\\x07b\n");
        assert_eq!(truncate("abcdef", 4), "abc\u{2026}");
    }
}

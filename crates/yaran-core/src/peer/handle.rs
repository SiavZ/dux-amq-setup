//! Agent-handle and operator-text helpers the peer router uses.
//!
//! There is one alphabet, one length bound and one sanitizer: these are the
//! canonical `crate::model` handle functions and `crate::sanitize` text
//! helpers, re-exported (the fork kept them in `model.rs`, `storage.rs` and
//! `sanitize.rs`). Only the two shapes the router needs differently live
//! here: [`derive_agent_handle`] takes an optional branch, and
//! [`suffixed_handle`] exposes the `-N` suffix rule for the global claim.

use std::collections::HashSet;

pub use crate::model::{AGENT_HANDLE_MAX_LEN, is_valid_agent_handle, normalize_agent_handle};
pub use crate::sanitize::{amq_handle, for_terminal, truncate, utf8_lossy};

/// A stable handle for a session: its directory basename, else its branch,
/// else its id, normalized. The canonical rule, with the branch optional.
pub fn derive_agent_handle(directory: &str, branch_name: Option<&str>, id: &str) -> String {
    crate::model::derive_agent_handle(directory, branch_name.unwrap_or(""), id)
}

/// `base` if it is free, otherwise `base-2`, `base-3`, ... within
/// [`AGENT_HANDLE_MAX_LEN`].
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

/// `base` with a `-N` suffix, the prefix cut so the whole stays within
/// [`AGENT_HANDLE_MAX_LEN`] bytes. Byte-for-byte the rule `SessionStore` uses
/// for local uniqueness, so a handle suffixed by either agrees.
pub(crate) fn suffixed_handle(base: &str, ordinal: u64) -> String {
    let suffix = format!("-{ordinal}");
    let prefix_len = AGENT_HANDLE_MAX_LEN.saturating_sub(suffix.len());
    let prefix = base.chars().take(prefix_len).collect::<String>();
    format!("{prefix}{suffix}")
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

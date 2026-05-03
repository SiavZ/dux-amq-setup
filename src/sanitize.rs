//! Strip ANSI/OSC/DCS/control bytes from operator-visible strings.
//!
//! Operator-trust strings (log lines, status messages, error popups) MUST
//! pass through this filter. Without it, an attacker who controls a git
//! stderr message, PR title, branch name, or process name can inject
//! escape sequences that rewrite the operator's terminal title (OSC 0/2),
//! drop covering OSC 8 hyperlinks, or paste-inject via OSC 52 the next
//! time `tail dux.log` is run. Same class as Rails CVE-2025-55193.
//!
//! IMPORTANT: this module is called from inside `crate::logger::log`.
//! It MUST NOT call any logging facility (`crate::logger::*`, `tracing::*`,
//! `eprintln!` is fine but discouraged) on any code path — doing so would
//! create an infinite recursion when the logger sanitizes its own input.

const SAFE_NEWLINE: char = '\n';
const SAFE_TAB: char = '\t';

/// Strip control bytes and ESC; preserve printable + `\t` + `\n`.
/// Replaces stripped bytes with their `\xNN` hex form so operators can
/// still see what was filtered (no silent loss).
pub fn for_terminal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            SAFE_NEWLINE | SAFE_TAB => out.push(c),
            c if c.is_control() => {
                use std::fmt::Write;
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c if (c as u32) == 0x7f => out.push_str("\\x7f"),
            c if (c as u32) == 0x9b => out.push_str("\\x9b"), // CSI 8-bit
            c => out.push(c),
        }
    }
    out
}

/// Convenience: like `String::from_utf8_lossy(...).to_string()` but also
/// runs `for_terminal`. Use for command stderr where bytes are bounded.
pub fn utf8_lossy(bytes: &[u8]) -> String {
    for_terminal(&String::from_utf8_lossy(bytes))
}

/// Truncate after sanitization so `\xNN` expansions don't overflow.
pub fn truncate(s: &str, max_chars: usize) -> String {
    let cleaned = for_terminal(s);
    if cleaned.chars().count() <= max_chars {
        cleaned
    } else {
        cleaned
            .chars()
            .take(max_chars - 1)
            .chain(std::iter::once('…'))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_osc_title_set() {
        let s = "\x1b]2;rm -rf $HOME\x07";
        let out = for_terminal(s);
        assert!(!out.contains('\x1b'));
        assert!(!out.contains('\x07'));
        assert!(out.contains("\\x1b"));
    }

    #[test]
    fn preserves_newlines_and_tabs() {
        assert_eq!(for_terminal("a\tb\nc"), "a\tb\nc");
    }

    #[test]
    fn handles_8bit_csi() {
        assert!(for_terminal("\u{009b}A").contains("\\x9b"));
    }

    #[test]
    fn utf8_lossy_handles_invalid_bytes() {
        let bytes = b"hello \xff\x1b]2;evil\x07 world";
        let out = utf8_lossy(bytes);
        assert!(!out.contains('\x1b'));
        assert!(out.contains("hello"));
        assert!(out.contains("world"));
    }

    #[test]
    fn truncate_with_ellipsis() {
        let s = "0123456789";
        assert_eq!(truncate(s, 5), "0123…");
    }
}

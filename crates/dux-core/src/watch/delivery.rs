//! How automatic text reaches an agent's PTY.
//!
//! Shared by the watch engine and (once ported) the AMQ inject drainer, which
//! is why these are free functions keyed by provider rather than engine
//! methods. The encoding is the fork's, proven against real Ink-based CLIs:
//!
//! - Claude and Codex get the body as an explicit bracketed paste, so an
//!   embedded newline stays inside the prompt instead of submitting it.
//! - Every other CLI gets the macro encoding (newline becomes Alt+Enter), the
//!   same bytes a manual macro sends.
//! - The submit key is a separate, later write (see
//!   [`effective_enter_phase_delay`]). Sent in the same chunk as the body, an
//!   Ink reader treats the trailing CR as part of a paste-shaped buffer and the
//!   text sits unsubmitted in the input field (claude-code #23513, #40168,
//!   #33987).

use std::time::Duration;

use crate::model::ProviderKind;

const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";
const RAW_ENTER: &[u8] = b"\r";

/// Minimum non-zero gap between the body bytes and the submit key. Smaller
/// values proved too small for typed-body harnesses under load: body and CR
/// could still be read as one paste-shaped buffer.
pub const MIN_ENTER_PHASE_DELAY_MS: u64 = 250;

/// The gap to wait before sending the submit key. `0` is an explicit escape
/// hatch for next-tick delivery; any other value is raised to
/// [`MIN_ENTER_PHASE_DELAY_MS`].
pub fn effective_enter_phase_delay(configured_ms: u64) -> Duration {
    if configured_ms == 0 {
        Duration::ZERO
    } else {
        Duration::from_millis(configured_ms.max(MIN_ENTER_PHASE_DELAY_MS))
    }
}

/// The body bytes for `body` typed into `provider`'s prompt. Never includes
/// the submit key; see [`submit_key_bytes_for_provider`].
pub fn inject_body_bytes_for_provider(body: &str, provider: Option<&ProviderKind>) -> Vec<u8> {
    match provider.map(ProviderKind::as_str) {
        Some(name) if name.eq_ignore_ascii_case("claude") || name.eq_ignore_ascii_case("codex") => {
            bracketed_paste_payload_bytes(body)
        }
        _ => crate::macros::macro_payload_bytes(body),
    }
}

/// The bytes that submit a prompt. A raw CR for every CLI dux knows today.
pub fn submit_key_bytes_for_provider(_provider: Option<&ProviderKind>) -> &'static [u8] {
    RAW_ENTER
}

fn bracketed_paste_payload_bytes(body: &str) -> Vec<u8> {
    let sanitized = sanitize_bracketed_paste_text(body);
    let mut payload = Vec::with_capacity(
        BRACKETED_PASTE_START.len() + sanitized.len() + BRACKETED_PASTE_END.len(),
    );
    payload.extend_from_slice(BRACKETED_PASTE_START);
    payload.extend_from_slice(sanitized.as_bytes());
    payload.extend_from_slice(BRACKETED_PASTE_END);
    payload
}

/// Normalize line endings to `\n` and render every other control character as
/// visible `\xNN`, so text can never close the paste early (an embedded
/// `ESC[201~`) or smuggle a keystroke.
fn sanitize_bracketed_paste_text(body: &str) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\n' | '\t' => out.push(ch),
            ch if ch.is_control() || ch == '\u{7f}' || ch == '\u{9b}' => {
                let _ = write!(out, "\\x{:02x}", ch as u32);
            }
            ch => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_body_uses_bracketed_paste() {
        let provider = ProviderKind::from_str("codex");
        assert_eq!(
            inject_body_bytes_for_provider("a\nb", Some(&provider)),
            b"\x1b[200~a\nb\x1b[201~"
        );
    }

    #[test]
    fn claude_long_body_uses_bracketed_paste() {
        let provider = ProviderKind::from_str("claude");
        let body = "x".repeat(1537);
        let payload = inject_body_bytes_for_provider(&body, Some(&provider));
        assert_eq!(&payload[..6], b"\x1b[200~");
        assert_eq!(&payload[6..payload.len() - 6], body.as_bytes());
        assert_eq!(&payload[payload.len() - 6..], b"\x1b[201~");
    }

    #[test]
    fn codex_body_normalizes_crlf_inside_bracketed_paste() {
        let provider = ProviderKind::from_str("codex");
        assert_eq!(
            inject_body_bytes_for_provider("a\r\nb\rc", Some(&provider)),
            b"\x1b[200~a\nb\nc\x1b[201~"
        );
    }

    #[test]
    fn codex_body_escapes_control_chars_inside_bracketed_paste() {
        let provider = ProviderKind::from_str("codex");
        assert_eq!(
            inject_body_bytes_for_provider("a\x1b[201~b", Some(&provider)),
            b"\x1b[200~a\\x1b[201~b\x1b[201~"
        );
    }

    #[test]
    fn non_bracketed_paste_body_uses_macro_payload_newline_encoding() {
        for name in ["gemini", "custom", "jcode"] {
            let provider = ProviderKind::from_str(name);
            assert_eq!(
                inject_body_bytes_for_provider("a\nb", Some(&provider)),
                b"a\x1b\rb"
            );
        }
        assert_eq!(inject_body_bytes_for_provider("a\nb", None), b"a\x1b\rb");
    }

    #[test]
    fn body_never_carries_the_submit_key() {
        for name in ["claude", "codex", "opencode"] {
            let provider = ProviderKind::from_str(name);
            let body = inject_body_bytes_for_provider("please continue", Some(&provider));
            assert!(!body.ends_with(b"\r"), "{name}: {body:?}");
        }
    }

    #[test]
    fn submit_key_is_raw_enter() {
        assert_eq!(
            submit_key_bytes_for_provider(Some(&ProviderKind::from_str("codex"))),
            b"\r"
        );
        assert_eq!(submit_key_bytes_for_provider(None), b"\r");
    }

    #[test]
    fn phase_delay_has_a_floor_but_zero_is_an_escape_hatch() {
        assert_eq!(effective_enter_phase_delay(0), Duration::ZERO);
        assert_eq!(effective_enter_phase_delay(1), Duration::from_millis(250));
        assert_eq!(effective_enter_phase_delay(900), Duration::from_millis(900));
    }
}

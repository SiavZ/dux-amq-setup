//! Claude Code on-disk project-dir encoder (Rust port).
//!
//! Single source of truth used by [`crate::purge`] (the GDPR hard purge) to
//! find the provider chat-history directory that belongs to a worktree. It must
//! produce byte-identical output to the `dux-amq/scripts/encode-claude-project-dir`
//! shell encoder: a drift would orphan provider chat dirs that a purge was
//! supposed to erase.
//!
//! ## Encoding rules
//!
//! Verified against Claude Code 2.1.111 on Linux. The reference fixture is
//! `tests/fixtures/claude-paths.txt` in this crate, a verbatim copy of the
//! overlay's `dux-amq/tests/fixtures/claude-paths.txt` that the bash encoder's
//! bats test consumes.
//!
//!   1. Strip a single trailing `/` if present (preserve `"/"` itself).
//!   2. Replace every character that is NOT in `[A-Za-z0-9-]` with `-`.
//!      This single rule covers `/`, `_`, `.`, space, parens, `@`, `+`,
//!      `:` and every other separator probed. Runs of unsafe chars are NOT
//!      collapsed: `__` becomes `--`, not `-`.
//!   3. Case is preserved (`Foo_Bar` becomes `Foo-Bar`).
//!
//! ## Invariants
//!
//! - Every absolute path produces some string. Empty or relative input is an
//!   error, because Claude Code only ever stores absolute paths and a relative
//!   input would silently name the wrong directory.
//! - Output contains only ASCII bytes from `[A-Za-z0-9-]`, which is
//!   filesystem-safe on every platform dux targets.

use std::path::Path;

/// Encode an absolute filesystem path the way Claude Code names its on-disk
/// session dirs at `~/.claude/projects/<encoded>`.
pub fn encode_claude_project_dir(path: &Path) -> anyhow::Result<String> {
    let raw = path.to_str().ok_or_else(|| {
        anyhow::anyhow!(
            "path is not valid UTF-8: {}",
            crate::sanitize::for_terminal(&path.display().to_string())
        )
    })?;
    encode_str(raw)
}

/// String-input variant of [`encode_claude_project_dir`], same rules.
pub fn encode_str(raw: &str) -> anyhow::Result<String> {
    if raw.is_empty() {
        anyhow::bail!("empty path is not encodable");
    }
    if !raw.starts_with('/') {
        anyhow::bail!(
            "absolute path required, got {:?}",
            crate::sanitize::for_terminal(raw)
        );
    }

    // Strip a single trailing `/`, but keep "/" itself: stripping it would
    // leave nothing to encode.
    let trimmed: &str = if raw == "/" {
        raw
    } else {
        raw.strip_suffix('/').unwrap_or(raw)
    };

    Ok(trimmed
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical fixture shared with the bash encoder. Lines starting with
    /// `#` and blank lines are skipped; the rest are TAB-separated
    /// `<input>\t<expected>` records.
    fn load_fixtures() -> Vec<(String, String)> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude-paths.txt");
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read fixture {}: {e}", path.display()));
        let mut out = Vec::new();
        for line in raw.lines() {
            let line = line.trim_end_matches('\r');
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let (input, expected) = line
                .split_once('\t')
                .unwrap_or_else(|| panic!("fixture line missing TAB separator: {line:?}"));
            out.push((input.to_string(), expected.to_string()));
        }
        assert!(
            out.len() >= 6,
            "claude-paths.txt should have at least 6 cases"
        );
        out
    }

    #[test]
    fn matches_canonical_fixtures() {
        for (input, expected) in load_fixtures() {
            let got =
                encode_str(&input).unwrap_or_else(|e| panic!("encode_str({input:?}) failed: {e}"));
            assert_eq!(got, expected, "fixture mismatch for input {input:?}");
        }
    }

    #[test]
    fn rejects_relative_paths() {
        assert!(encode_str("relative/path").is_err());
        assert!(encode_str("foo").is_err());
    }

    #[test]
    fn rejects_empty_input() {
        assert!(encode_str("").is_err());
    }

    #[test]
    fn root_path_preserved_as_dash() {
        assert_eq!(encode_str("/").unwrap(), "-");
    }

    #[test]
    fn trailing_slash_stripped_then_encoded() {
        assert_eq!(
            encode_str("/tmp/probe/trailing/").unwrap(),
            "-tmp-probe-trailing"
        );
    }

    #[test]
    fn case_preserved() {
        assert_eq!(encode_str("/foo/MixedCase").unwrap(), "-foo-MixedCase");
    }

    #[test]
    fn runs_of_unsafe_chars_not_collapsed() {
        assert_eq!(
            encode_str("/foo/double__under").unwrap(),
            "-foo-double--under"
        );
        assert_eq!(
            encode_str("/foo/with..dotdot").unwrap(),
            "-foo-with--dotdot"
        );
    }

    #[test]
    fn path_input_variant_works() {
        let p = Path::new("/tmp/probe/with-dash");
        assert_eq!(
            encode_claude_project_dir(p).unwrap(),
            "-tmp-probe-with-dash"
        );
    }
}

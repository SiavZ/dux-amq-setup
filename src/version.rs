//! Build-stamped version information.
//!
//! `build.rs` embeds the short git commit of the checkout the binary was
//! built from (plus a `-dirty` suffix for uncommitted tracked changes)
//! via the `DUX_GIT_COMMIT` env var. Outside a git checkout the stamp
//! falls back to `unknown`. The Cargo package version alone is not a
//! reliable build identifier — it is rarely bumped between commits — so
//! every user-facing version string should go through [`long`].

/// Cargo package version, e.g. `0.1.0`.
pub const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Short git commit the binary was built from (`unknown` when built
/// outside a git checkout).
pub const GIT_COMMIT: &str = env!("DUX_GIT_COMMIT");

/// Human-facing version label, e.g. `0.1.0+70cfd17`.
pub fn long() -> String {
    format!("{PKG_VERSION}+{GIT_COMMIT}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_embeds_pkg_version_and_commit() {
        let v = long();
        assert!(v.starts_with(PKG_VERSION));
        assert!(v.ends_with(GIT_COMMIT));
        // build.rs always emits a non-empty stamp ("unknown" fallback),
        // so the label must carry something after the `+` separator.
        let stamp = v.split('+').nth(1).unwrap_or("");
        assert!(!stamp.is_empty());
    }
}

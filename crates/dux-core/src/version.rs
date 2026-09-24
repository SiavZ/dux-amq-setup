//! Build-stamped version information.
//!
//! [`crate::display_version`] is what every surface shows as "the version"
//! (`vX.Y.Z` for a release build, `development` otherwise), and the what's-new
//! screen keys on it, so it must not change per commit. It also cannot tell two
//! development builds apart, which made "is this the binary I just built?"
//! needlessly hard to answer. `build.rs` therefore also embeds the short git
//! commit (with `-dirty` for uncommitted tracked changes, `unknown` outside a
//! checkout), and [`long`] combines the two for the places that identify a
//! BUILD: `dux --version`, the log's bootstrap line and the TUI header.

/// Short git commit the binary was built from, e.g. `70cfd17` or
/// `70cfd17-dirty`; `unknown` when built outside a git checkout.
pub const GIT_COMMIT: &str = env!("DUX_GIT_COMMIT");

/// The display version plus the commit, e.g. `development+70cfd17` or
/// `v0.1.0+70cfd17`.
pub fn long() -> String {
    format!("{}+{GIT_COMMIT}", crate::display_version())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fork name (3dd80427). The fork stamped the Cargo package version;
    /// here the prefix is `display_version()`, which is what every surface
    /// shows. build.rs always emits a non-empty stamp ("unknown" outside a
    /// checkout), so something must follow the `+`.
    #[test]
    fn long_embeds_pkg_version_and_commit() {
        let v = long();
        assert!(v.starts_with(crate::display_version()));
        assert!(v.ends_with(GIT_COMMIT));
        let stamp = v.split('+').nth(1).unwrap_or("");
        assert!(!stamp.is_empty());
    }

    #[test]
    fn long_embeds_display_version_and_commit() {
        let v = long();
        assert!(v.starts_with(crate::display_version()), "{v}");
        assert!(v.ends_with(GIT_COMMIT), "{v}");
        // build.rs always emits a non-empty stamp ("unknown" fallback), so the
        // label always carries something after the `+`.
        let stamp = v.rsplit('+').next().unwrap_or("");
        assert!(!stamp.is_empty(), "{v}");
    }

    /// Built from this repository, so the stamp is a real short hash, not the
    /// fallback. Guards the build script's path handling: it runs from a crate
    /// two levels below the repo root, often in a linked worktree where `.git`
    /// is a file.
    #[test]
    fn the_stamp_is_a_real_commit_in_a_git_checkout() {
        let hash = GIT_COMMIT.trim_end_matches("-dirty");
        assert_eq!(hash.len(), 7, "{GIT_COMMIT}");
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()), "{GIT_COMMIT}");
    }
}

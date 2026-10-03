//! THE single home for every yaran URL that any surface links to.
//!
//! Both the TUI and the web UI render these, so a link that lives in only one
//! surface's source is a link that will eventually disagree with the other.
//! Add a constant here rather than a string literal in a component.

/// The project homepage until a Yaran website is deployed.
pub const WEBSITE: &str = REPO;

/// The documentation source in the actual project repository.
pub const DOCS: &str = "https://github.com/SiavZ/dux-amq-setup/tree/main/website/docs";

/// `owner/repo`, the one place the coordinates are spelled out. Everything
/// below is built from it so a rename is a one-line change.
pub const REPO_SLUG: &str = "SiavZ/dux-amq-setup";

/// The source repository.
pub const REPO: &str = "https://github.com/SiavZ/dux-amq-setup";

/// The releases index. Also the fallback destination when there is no release
/// matching the running version (a dev build, or a tag GitHub has not published
/// a release for).
pub const RELEASES: &str = "https://github.com/SiavZ/dux-amq-setup/releases";

/// The GitHub REST API root. Injectable at the call site so tests can point a
/// fetcher at a local server instead of the network.
pub const GITHUB_API_BASE: &str = "https://api.github.com";

/// The web page for one release tag, e.g. `v0.6.0`.
///
/// Prefer the `html_url` the API hands back when you have it; this is for the
/// case where only a version string is in hand.
pub fn release_tag(version: &str) -> String {
    format!("{RELEASES}/tag/{version}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_url_is_https_and_has_no_trailing_slash() {
        for url in [WEBSITE, DOCS, REPO, RELEASES, GITHUB_API_BASE] {
            assert!(url.starts_with("https://"), "{url} is not https");
            assert!(!url.ends_with('/'), "{url} has a trailing slash");
        }
    }

    #[test]
    fn repo_urls_are_built_from_the_one_slug() {
        assert!(REPO.ends_with(REPO_SLUG), "{REPO} must name {REPO_SLUG}");
        assert_eq!(RELEASES, format!("{REPO}/releases"));
    }

    #[test]
    fn product_links_do_not_claim_upstreams_website() {
        assert_eq!(WEBSITE, REPO);
        assert_eq!(DOCS, format!("{REPO}/tree/main/website/docs"));
        assert!(!WEBSITE.contains("getdux.app"));
        assert!(!DOCS.contains("getdux.app"));
    }

    #[test]
    fn release_tag_points_at_the_versions_own_page() {
        assert_eq!(
            release_tag("v0.6.0"),
            "https://github.com/SiavZ/dux-amq-setup/releases/tag/v0.6.0"
        );
    }
}

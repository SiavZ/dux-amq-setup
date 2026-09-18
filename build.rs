//! Embed the git commit of the build checkout into the binary so the TUI
//! header and `dux --version` can distinguish builds that all share the
//! same (rarely bumped) Cargo package version.

use std::process::Command;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());

    let commit = git(&manifest_dir, &["rev-parse", "--short=7", "HEAD"]);
    let dirty = git(
        &manifest_dir,
        &["status", "--porcelain", "--untracked-files=no"],
    )
    .map(|out| !out.is_empty());

    // The `-dirty` suffix is best-effort: rerun-if-changed below only
    // watches .git, so editing a tracked file without committing does not
    // re-stamp until the next rebuild of build.rs's inputs. Commits and
    // branch switches always re-stamp, which is the signal that matters.
    let stamp = match (commit, dirty) {
        (Some(c), Some(true)) => format!("{c}-dirty"),
        (Some(c), _) => c,
        (None, _) => "unknown".to_string(),
    };
    println!("cargo:rustc-env=DUX_GIT_COMMIT={stamp}");

    println!("cargo:rerun-if-changed={manifest_dir}/.git/HEAD");
    if let Ok(head) = std::fs::read_to_string(format!("{manifest_dir}/.git/HEAD")) {
        if let Some(reference) = head.strip_prefix("ref: ") {
            println!(
                "cargo:rerun-if-changed={manifest_dir}/.git/{}",
                reference.trim()
            );
        }
    }
}

fn git(dir: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

use std::env;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=DUX_RELEASE_BUILD");
    let display_version = if env::var("DUX_RELEASE_BUILD").as_deref() == Ok("1") {
        format!("v{}", env!("CARGO_PKG_VERSION"))
    } else {
        "development".to_string()
    };
    println!("cargo:rustc-env=DUX_DISPLAY_VERSION={display_version}");

    stamp_git_commit();
}

/// Embed the short git commit the binary was built from as `DUX_GIT_COMMIT`,
/// with a `-dirty` suffix when tracked files differ from it, or `unknown`
/// outside a git checkout. The package version is rarely bumped between
/// commits, so it alone cannot tell two builds apart; see `crate::version`.
///
/// Paths are asked of git (`rev-parse --git-path`) rather than built as
/// `<crate>/.git/...`: this crate sits two levels below the repository root,
/// and in a linked worktree `.git` is a file, not a directory.
fn stamp_git_commit() {
    let dir = env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());
    let commit = git(&dir, &["rev-parse", "--short=7", "HEAD"]);
    let dirty =
        git(&dir, &["status", "--porcelain", "--untracked-files=no"]).map(|out| !out.is_empty());
    let stamp = match (commit, dirty) {
        (Some(c), Some(true)) => format!("{c}-dirty"),
        (Some(c), _) => c,
        (None, _) => "unknown".to_string(),
    };
    println!("cargo:rustc-env=DUX_GIT_COMMIT={stamp}");

    // Re-stamp on a commit, a checkout, or a ref pack. The `-dirty` suffix is
    // best-effort: editing a tracked file without committing does not re-run
    // this script until one of these changes. The index is deliberately NOT
    // watched: the `git status` above refreshes it, so watching it would re-run
    // this script (and rebuild dux-core) on every single build.
    for path in ["HEAD", "packed-refs"] {
        if let Some(p) = git(&dir, &["rev-parse", "--git-path", path]) {
            println!("cargo:rerun-if-changed={}", absolute(&dir, &p));
        }
    }
    if let Some(reference) = git(&dir, &["symbolic-ref", "-q", "HEAD"])
        && let Some(p) = git(&dir, &["rev-parse", "--git-path", &reference])
    {
        println!("cargo:rerun-if-changed={}", absolute(&dir, &p));
    }
}

fn absolute(dir: &str, path: &str) -> String {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        path.to_string()
    } else {
        std::path::Path::new(dir).join(p).display().to_string()
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
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

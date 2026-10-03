//! Verify the real build script, including new-name precedence and legacy input.
use std::process::Command;

#[test]
fn release_stamp_accepts_legacy_input_without_ignoring_present_primary_values() {
    let temp = tempfile::tempdir().expect("scratch directory");
    let executable = temp.path().join("core-build-script");
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let compiled = Command::new(rustc)
        .args(["--edition=2024", "build.rs", "-o"])
        .arg(&executable)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("CARGO_PKG_VERSION", env!("CARGO_PKG_VERSION"))
        .output()
        .expect("compile actual build script");
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );

    let release_version = format!("v{}", env!("CARGO_PKG_VERSION"));
    for (primary, legacy, expected) in [
        (None, None, "development"),
        (None, Some("1"), release_version.as_str()),
        (Some("1"), Some("0"), release_version.as_str()),
        (Some("0"), Some("1"), "development"),
        (Some(""), Some("1"), "development"),
    ] {
        let mut command = Command::new(&executable);
        command
            .current_dir(temp.path())
            .env_remove("YARAN_RELEASE_BUILD")
            .env_remove("DUX_RELEASE_BUILD")
            .env("CARGO_MANIFEST_DIR", temp.path());
        if let Some(value) = primary {
            command.env("YARAN_RELEASE_BUILD", value);
        }
        if let Some(value) = legacy {
            command.env("DUX_RELEASE_BUILD", value);
        }
        let output = command.output().expect("execute actual build script");
        assert!(output.status.success());
        let stdout = String::from_utf8(output.stdout).expect("build script output");
        assert!(
            stdout
                .lines()
                .any(|line| line == format!("cargo:rustc-env=YARAN_DISPLAY_VERSION={expected}")),
            "primary={primary:?}, legacy={legacy:?}: {stdout}"
        );
        assert!(stdout.contains("cargo:rerun-if-env-changed=DUX_RELEASE_BUILD"));
    }
}

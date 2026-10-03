//! `yaran --version` runs the real binary and names the commit it was built
//! from, before touching config or the single-instance lock.

use std::process::Command;

#[test]
fn version_flag_prints_the_display_version_and_commit() {
    let home = tempfile::tempdir().expect("tempdir");
    // A YARAN_HOME that does not exist yet: the flag must not create or need it.
    let yaran_home = home.path().join("absent");
    for flag in ["--version", "-V"] {
        let out = Command::new(env!("CARGO_BIN_EXE_yaran"))
            .arg(flag)
            .env("YARAN_HOME", &yaran_home)
            .output()
            .expect("run yaran");
        assert!(out.status.success(), "{flag}: {out:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(
            stdout.trim(),
            format!("yaran {}", yaran_core::version::long()),
            "{flag}"
        );
        assert!(!yaran_home.exists(), "{flag} must not create YARAN_HOME");
    }
}

#[test]
fn help_uses_the_new_command_without_initializing_either_brand_home() {
    let home = tempfile::tempdir().expect("tempdir");
    let yaran_home = home.path().join("yaran-absent");
    let dux_home = home.path().join("dux-absent");
    for args in [vec!["--help"], vec!["server", "--help"]] {
        let out = Command::new(env!("CARGO_BIN_EXE_yaran"))
            .args(&args)
            .env("YARAN_HOME", &yaran_home)
            .env("DUX_HOME", &dux_home)
            .output()
            .expect("run yaran help");
        assert!(out.status.success(), "{args:?}: {out:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("yaran"), "{stdout}");
        assert!(!stdout.contains("dux"), "legacy branding in help: {stdout}");
        assert!(!yaran_home.exists(), "help must not create YARAN_HOME");
        assert!(!dux_home.exists(), "help must not create DUX_HOME");
    }
}

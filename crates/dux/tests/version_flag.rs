//! `dux --version` runs the real binary and names the commit it was built
//! from, before touching config or the single-instance lock.

use std::process::Command;

#[test]
fn version_flag_prints_the_display_version_and_commit() {
    let home = tempfile::tempdir().expect("tempdir");
    // A DUX_HOME that does not exist yet: the flag must not create or need it.
    let dux_home = home.path().join("absent");
    for flag in ["--version", "-V"] {
        let out = Command::new(env!("CARGO_BIN_EXE_dux"))
            .arg(flag)
            .env("DUX_HOME", &dux_home)
            .output()
            .expect("run dux");
        assert!(out.status.success(), "{flag}: {out:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(
            stdout.trim(),
            format!("dux {}", dux_core::version::long()),
            "{flag}"
        );
        assert!(!dux_home.exists(), "{flag} must not create DUX_HOME");
    }
}

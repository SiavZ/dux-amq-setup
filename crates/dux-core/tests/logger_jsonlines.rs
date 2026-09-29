//! Integration test: `logger::init` writes JSON Lines records to `dux.log`, and
//! both `tracing::*!` macros and the `dux_core::logger::*` free functions funnel
//! into the same file.
//!
//! This is the contract the fork's log consumers rely on: the purge redacts
//! records by `fields.session_id` and the doctor filters on `.level`. Its own
//! binary because `init` installs a process-global logger.

use std::path::PathBuf;

use dux_core::config::{DuxPaths, LoggingConfig};

fn fake_paths(root: PathBuf) -> DuxPaths {
    DuxPaths {
        config_path: root.join("config.toml"),
        sessions_db_path: root.join("sessions.sqlite3"),
        worktrees_root: root.join("worktrees"),
        lock_path: root.join("dux.lock"),
        root,
    }
}

#[test]
fn init_emits_json_lines_with_target_and_fields() {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = fake_paths(dir.path().to_path_buf());
    let cfg = LoggingConfig {
        level: "info".to_string(),
        path: String::new(),
        ..LoggingConfig::default()
    };

    dux_core::logger::init(&cfg, &paths);

    tracing::info!(
        target: "dux::probe",
        session_id = "demo",
        n = 3i64,
        "hello world",
    );
    // Below the configured level: must not be written.
    tracing::debug!(target: "dux::probe", "too quiet");
    // Free function: must also reach the file, sanitized.
    dux_core::logger::error("legacy with \x1b[31mansi\x1b[0m bytes");

    // The writer is synchronous, so the lines are on disk already.
    let text = std::fs::read_to_string(dir.path().join("dux.log")).expect("read dux.log");
    assert!(!text.is_empty(), "log file is empty");

    let mut saw_probe = false;
    let mut saw_legacy_sanitized = false;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let parsed: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("non-JSON line: {line:?} ({e})"));
        assert!(parsed.get("timestamp").is_some(), "no timestamp: {line}");
        assert!(parsed.get("level").is_some(), "no level: {line}");
        let target = parsed.get("target").and_then(|v| v.as_str()).unwrap_or("");
        let message = parsed
            .get("fields")
            .and_then(|f| f.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("");
        assert_ne!(message, "too quiet", "a debug line passed an info filter");
        if target == "dux::probe" {
            saw_probe = true;
            let fields = parsed.get("fields").expect("fields object");
            assert_eq!(
                fields.get("session_id").and_then(|v| v.as_str()),
                Some("demo")
            );
            assert_eq!(fields.get("n").and_then(|v| v.as_i64()), Some(3));
            assert_eq!(message, "hello world");
            assert_eq!(parsed["level"], "INFO");
        }
        if target == "dux::legacy" && message.starts_with("legacy with") {
            saw_legacy_sanitized = true;
            assert_eq!(parsed["level"], "ERROR");
            assert!(
                !message.contains('\u{1b}'),
                "legacy shim leaked ESC bytes into log: {message}"
            );
            assert!(
                message.contains("\\x1b"),
                "sanitizer should rewrite ESC as \\xNN; got {message}"
            );
        }
    }
    assert!(saw_probe, "expected a dux::probe record in {text}");
    assert!(saw_legacy_sanitized, "expected the legacy record in {text}");
}

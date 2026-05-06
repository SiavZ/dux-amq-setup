use std::thread;
use std::time::Duration;

/// Smoke test: verify that spawning a simple command via PTY works
/// by checking that the process exits cleanly.
#[test]
fn pty_spawn_and_detect_exit() {
    // We cannot import PtyClient directly (private module), so we test
    // the underlying portable-pty crate to ensure it works on this platform.
    use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

    let pty_system = NativePtySystem::default();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("failed to open PTY");

    let mut cmd = CommandBuilder::new("echo");
    cmd.arg("hello-from-pty");

    let mut child = pair.slave.spawn_command(cmd).expect("failed to spawn");
    drop(pair.slave);

    // Wait for exit.
    let status = child.wait().expect("failed to wait");
    assert!(status.success());
}

/// Verify that PTY output can be read and parsed by alacritty_terminal.
#[test]
fn pty_read_output_into_alacritty_terminal() {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::term::{self, Config, Term};
    use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
    use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
    use std::io::Read;

    struct TerminalDimensions {
        rows: usize,
        cols: usize,
    }

    impl Dimensions for TerminalDimensions {
        fn total_lines(&self) -> usize {
            self.rows
        }

        fn screen_lines(&self) -> usize {
            self.rows
        }

        fn columns(&self) -> usize {
            self.cols
        }
    }

    let pty_system = NativePtySystem::default();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("failed to open PTY");

    let mut cmd = CommandBuilder::new("echo");
    cmd.arg("hello-from-pty");

    let mut child = pair.slave.spawn_command(cmd).expect("failed to spawn");
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .expect("failed to clone reader");
    let mut parser: Processor<StdSyncHandler> = Processor::new();
    let dimensions = TerminalDimensions { rows: 24, cols: 80 };
    let mut term = Term::new(Config::default(), &dimensions, VoidListener);

    // Read output in a loop until EOF.
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => parser.advance(&mut term, &buf[..n]),
            Err(_) => break,
        }
    }

    child.wait().expect("failed to wait");

    let renderable = term.renderable_content();
    let mut viewport = vec![String::new(); 24];
    for indexed in renderable.display_iter {
        let Some(point) = term::point_to_viewport(renderable.display_offset, indexed.point) else {
            continue;
        };
        let row = &mut viewport[point.line];
        while row.len() < indexed.point.column.0 {
            row.push(' ');
        }
        row.push(indexed.cell.c);
        if let Some(zerowidth) = indexed.cell.zerowidth() {
            for ch in zerowidth {
                row.push(*ch);
            }
        }
    }
    assert!(
        viewport.iter().any(|line| line.contains("hello-from-pty")),
        "Expected 'hello-from-pty' in terminal output"
    );
}

/// Verify that writing to the PTY sends input to the child.
#[test]
fn pty_write_input() {
    use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
    use std::io::{Read, Write};
    use std::sync::mpsc;
    use std::time::Instant;

    let pty_system = NativePtySystem::default();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("failed to open PTY");

    // Use `cat` which echoes stdin to stdout.
    let cmd = CommandBuilder::new("cat");
    let mut child = pair.slave.spawn_command(cmd).expect("failed to spawn");
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().expect("reader");
    let mut writer = pair.master.take_writer().expect("writer");

    // Write some text followed by EOF (Ctrl-D).
    writer.write_all(b"test-input\n").expect("write");
    writer.write_all(b"\x04").expect("write eof");
    let _ = writer.flush();

    // The PTY reader is a blocking `Box<dyn Read + Send>`. To avoid racing
    // the kill against the child's echo on slow runners (notably macOS-14
    // GitHub runners, where the original write→sleep(200ms)→kill→read flow
    // could land the kill before `cat` had echoed bytes back through the
    // master), we drain on a background thread and poll the channel until
    // either we observe "test-input" in the accumulated output or a 5s
    // deadline elapses. The kill is issued *after* we've seen the bytes
    // (or timed out), so we never starve the read.
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => return,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
    });

    let needle = b"test-input";
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output: Vec<u8> = Vec::new();
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let chunk_timeout = std::cmp::min(remaining, Duration::from_millis(100));
        match rx.recv_timeout(chunk_timeout) {
            Ok(chunk) => {
                output.extend_from_slice(&chunk);
                if output.windows(needle.len()).any(|w| w == needle) {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let _ = child.kill();
    let text = String::from_utf8_lossy(&output);

    // The output should contain our input echoed back.
    assert!(
        text.contains("test-input"),
        "Expected 'test-input' in output (after 5s polling), got: {text}"
    );
}

/// audit03 Phase 3: `PtyClient::spawn_with_env` propagates the
/// per-session env vars to the child, while the global terminal-env
/// vars (`TERM`, `DUX_PANE`) still apply. Spawns a small shell that
/// prints the relevant env vars and asserts they're present in the
/// terminal output.
#[test]
fn spawn_with_env_propagates_per_session_vars() {
    use dux::model::{ProviderKind, SessionSettings};
    use dux::pty::PtyClient;
    use std::time::Instant;

    // Build a SessionSettings that should yield CLAUDE_AMQ_YOLO=1 and
    // DUX_AMQ_VERIFY=1. Worker mode + auto_clear are unrelated to env.
    let settings = SessionSettings {
        yolo_permissions: true,
        verify_envelope_override: Some(true),
        ..SessionSettings::default()
    };
    let provider = ProviderKind::new("claude");
    // verify_envelope_global=false; the override should win.
    let env = settings.to_pty_env(&provider, false);

    let args = [
        "-c".to_string(),
        // Print each env var on its own line so the assert below is
        // unambiguous regardless of shell quoting.
        "printf 'CY=%s\\nDV=%s\\nDP=%s\\n' \
         \"$CLAUDE_AMQ_YOLO\" \"$DUX_AMQ_VERIFY\" \"$DUX_PANE\""
            .to_string(),
    ];
    let cwd = std::path::Path::new("/tmp");
    let pty = PtyClient::spawn_with_env("/bin/sh", &args, cwd, 24, 80, 1_000, env)
        .expect("spawn_with_env");

    // Poll the recent-lines snapshot until we see all three markers
    // or hit the deadline. The reader thread is async so the first
    // snapshot may be empty.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut snapshot = String::new();
    while Instant::now() < deadline {
        snapshot = pty.scan_recent_lines(30);
        if snapshot.contains("CY=1") && snapshot.contains("DV=1") && snapshot.contains("DP=1") {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    assert!(
        snapshot.contains("CY=1"),
        "expected CLAUDE_AMQ_YOLO=1 in PTY output; got: {snapshot}"
    );
    assert!(
        snapshot.contains("DV=1"),
        "expected DUX_AMQ_VERIFY=1 in PTY output; got: {snapshot}"
    );
    assert!(
        snapshot.contains("DP=1"),
        "expected DUX_PANE=1 (terminal env) still present; got: {snapshot}"
    );
}

/// audit03 Phase 3: `verify_envelope_override = None` falls back to
/// the `verify_envelope_global` argument. Asserts both branches —
/// global=true and global=false — render the right `DUX_AMQ_VERIFY`
/// value at the child.
#[test]
fn spawn_with_env_falls_back_to_global_verify_envelope() {
    use dux::model::{ProviderKind, SessionSettings};

    let settings = SessionSettings::default();
    let provider = ProviderKind::new("claude");

    let env_strict = settings.to_pty_env(&provider, true);
    assert!(
        env_strict
            .vars
            .iter()
            .any(|(k, v)| k == "DUX_AMQ_VERIFY" && v == "1"),
        "global=true should yield DUX_AMQ_VERIFY=1; got {:?}",
        env_strict.vars
    );

    let env_skip = settings.to_pty_env(&provider, false);
    assert!(
        env_skip
            .vars
            .iter()
            .any(|(k, v)| k == "DUX_AMQ_VERIFY" && v == "0"),
        "global=false should yield DUX_AMQ_VERIFY=0; got {:?}",
        env_skip.vars
    );

    // Per-session override beats global in both directions.
    let overridden = SessionSettings {
        verify_envelope_override: Some(false),
        ..SessionSettings::default()
    };
    let env_skip_override = overridden.to_pty_env(&provider, true);
    assert!(
        env_skip_override
            .vars
            .iter()
            .any(|(k, v)| k == "DUX_AMQ_VERIFY" && v == "0"),
        "Some(false) override should yield DUX_AMQ_VERIFY=0 even when global=true"
    );
}

/// audit03 Phase 01 §15: per-session `system_prompt` propagates to
/// `DUX_SYSTEM_PROMPT` only when present and non-blank. Empty,
/// whitespace-only, and `None` values must NOT export the var (fail-safe
/// default — claude's `--append-system-prompt ""` would still alter the
/// model's prompt with empty content).
#[test]
fn to_pty_env_emits_system_prompt_only_when_set_and_non_blank() {
    use dux::model::{ProviderKind, SessionSettings};

    let provider = ProviderKind::new("claude");

    // None: no env var.
    let env_none = SessionSettings::default().to_pty_env(&provider, false);
    assert!(
        !env_none.vars.iter().any(|(k, _)| k == "DUX_SYSTEM_PROMPT"),
        "None system_prompt must not export DUX_SYSTEM_PROMPT"
    );

    // Some(""): no env var (treated as None).
    let env_empty = SessionSettings {
        system_prompt: Some(String::new()),
        ..SessionSettings::default()
    }
    .to_pty_env(&provider, false);
    assert!(
        !env_empty.vars.iter().any(|(k, _)| k == "DUX_SYSTEM_PROMPT"),
        "empty system_prompt must not export DUX_SYSTEM_PROMPT"
    );

    // Some("   \n\t  "): no env var (whitespace-only).
    let env_blank = SessionSettings {
        system_prompt: Some("   \n\t  ".into()),
        ..SessionSettings::default()
    }
    .to_pty_env(&provider, false);
    assert!(
        !env_blank.vars.iter().any(|(k, _)| k == "DUX_SYSTEM_PROMPT"),
        "whitespace-only system_prompt must not export DUX_SYSTEM_PROMPT"
    );

    // Some("be helpful"): exports the literal value.
    let env_set = SessionSettings {
        system_prompt: Some("be helpful".into()),
        ..SessionSettings::default()
    }
    .to_pty_env(&provider, false);
    assert!(
        env_set
            .vars
            .iter()
            .any(|(k, v)| k == "DUX_SYSTEM_PROMPT" && v == "be helpful"),
        "non-blank system_prompt must export DUX_SYSTEM_PROMPT verbatim, got {:?}",
        env_set.vars
    );

    // Multi-line with embedded newlines: passed through verbatim.
    let env_multi = SessionSettings {
        system_prompt: Some("line one\nline two".into()),
        ..SessionSettings::default()
    }
    .to_pty_env(&provider, false);
    assert!(
        env_multi
            .vars
            .iter()
            .any(|(k, v)| k == "DUX_SYSTEM_PROMPT" && v == "line one\nline two"),
        "multi-line system_prompt must round-trip embedded newlines"
    );
}

/// audit03 Phase 01 §15: `DUX_SYSTEM_PROMPT` is emitted regardless of
/// provider — the wrappers (`claude-amq`, `codex-amq`, `gemini-amq`)
/// decide whether to translate it into a CLI flag or warn-and-drop. dux
/// must not pre-filter by provider; that decision lives in the wrapper.
#[test]
fn to_pty_env_emits_system_prompt_for_every_provider() {
    use dux::model::{ProviderKind, SessionSettings};

    let settings = SessionSettings {
        system_prompt: Some("custom".into()),
        ..SessionSettings::default()
    };
    for provider_name in ["claude", "codex", "gemini", "opencode"] {
        let provider = ProviderKind::new(provider_name);
        let env = settings.to_pty_env(&provider, false);
        assert!(
            env.vars
                .iter()
                .any(|(k, v)| k == "DUX_SYSTEM_PROMPT" && v == "custom"),
            "expected DUX_SYSTEM_PROMPT=custom for provider {provider_name}; got {:?}",
            env.vars
        );
    }
}

/// Verify PTY resize doesn't panic.
#[test]
fn pty_resize() {
    use portable_pty::{NativePtySystem, PtySize, PtySystem};

    let pty_system = NativePtySystem::default();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("failed to open PTY");

    // Resize should not panic.
    pair.master
        .resize(PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("resize should succeed");
}

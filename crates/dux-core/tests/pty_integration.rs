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

    // Drain on a thread and wait for the echo, up to 5s, BEFORE the kill
    // (fork 411a59c2). The old write, sleep(200ms), kill, read order let the
    // kill land before `cat` had echoed on a slow runner (macOS-14 CI), which
    // read zero bytes.
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                return;
            }
        }
    });
    let needle = b"test-input";
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut output: Vec<u8> = Vec::new();
    while std::time::Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(chunk) => {
                output.extend_from_slice(&chunk);
                if output.windows(needle.len()).any(|w| w == needle) {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = child.kill();
    let text = String::from_utf8_lossy(&output);

    // The output should contain our input echoed back.
    assert!(
        text.contains("test-input"),
        "Expected 'test-input' in output, got: {text}"
    );
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

/// Fork name. Per-session settings (YOLO, verify override) turned into env by
/// `SessionSettings::to_pty_env` must reach the child through a real PTY,
/// alongside the terminal env the PTY layer adds itself.
#[test]
fn spawn_with_env_propagates_per_session_vars() {
    use dux_core::model::ProviderKind;
    use dux_core::pty::PtyClient;
    use dux_core::session_settings::SessionSettings;
    use std::time::Instant;

    let settings = SessionSettings {
        yolo_permissions: true,
        verify_envelope_override: Some(true),
        ..SessionSettings::default()
    };
    // Global verify off: the per-session override must win.
    let env = settings.to_pty_env(&ProviderKind::new("claude"), false);

    let args = [
        "-c".to_string(),
        "printf 'CY=%s\\nDV=%s\\nTM=%s\\n' \"$CLAUDE_AMQ_YOLO\" \"$DUX_AMQ_VERIFY\" \"${TERM:+set}\""
            .to_string(),
    ];
    let pty = PtyClient::spawn_with_env(
        "/bin/sh",
        &args,
        std::path::Path::new("/tmp"),
        24,
        80,
        1_000,
        &env.vars,
    )
    .expect("spawn_with_env");

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut snapshot = String::new();
    while Instant::now() < deadline {
        snapshot = pty.scan_recent_lines(30);
        if snapshot.contains("CY=1") && snapshot.contains("DV=1") && snapshot.contains("TM=set") {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(snapshot.contains("CY=1"), "CLAUDE_AMQ_YOLO: {snapshot}");
    assert!(snapshot.contains("DV=1"), "DUX_AMQ_VERIFY: {snapshot}");
    assert!(
        snapshot.contains("TM=set"),
        "the PTY's own TERM must survive the extra env: {snapshot}"
    );
}

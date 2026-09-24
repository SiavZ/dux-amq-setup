use std::sync::mpsc;

use anyhow::{Result, anyhow};
use dux_core::engine::{Final, StatusUpdate, status_op};

use crate::app::WorkerEvent;

/// Request sent from the main thread to the clipboard worker. Carries the
/// op's success and failure finals (already keyed by the StatusOp minted at
/// the call site), so the worker can ship back the resolved `status` without
/// the engine having to know the per-call label.
struct CopyRequest {
    text: String,
    label: String,
    op: dux_core::engine::StatusOp<(), String>,
    worker_tx: mpsc::Sender<WorkerEvent>,
}

/// Handle for sending clipboard copy requests to a long-lived background
/// thread. The background thread owns the `arboard::Clipboard` instance so
/// it stays alive for the entire app lifetime. This is required on X11
/// where the clipboard owner must remain running to serve paste requests.
pub(crate) struct Clipboard {
    tx: mpsc::Sender<CopyRequest>,
}

impl Clipboard {
    pub(crate) fn new() -> Self {
        let (tx, rx) = mpsc::channel::<CopyRequest>();

        std::thread::Builder::new()
            .name("clipboard".into())
            .spawn(move || {
                clipboard_worker(rx);
            })
            .expect("failed to spawn clipboard worker thread");

        Self { tx }
    }

    /// Send a clipboard copy request. Returns immediately with the keyed
    /// pending [`StatusUpdate`] for the busy spinner; the matching final
    /// arrives later as a `WorkerEvent::ClipboardCopyCompleted` carrying the
    /// resolved status. Callers that want the spinner apply the returned
    /// pending; fire-and-forget callers can drop it.
    ///
    /// `label` is the human-readable success message shown in the status bar
    /// when the copy completes.
    pub(crate) fn copy_text(
        &self,
        text: &str,
        label: &str,
        worker_tx: &mpsc::Sender<WorkerEvent>,
    ) -> Result<StatusUpdate> {
        // Declare the loading→final states together. The success message is the
        // label; the failure message matches the engine's prior wording.
        let success_label = label.to_string();
        let op = status_op("Copying path to clipboard\u{2026}")
            .on_success(move |_: &()| Final::info(success_label.clone()))
            .on_failure(|e: &String| Final::error(format!("Clipboard copy failed: {e}")));
        let pending = op.pending_status();
        self.tx
            .send(CopyRequest {
                text: text.to_string(),
                label: label.to_string(),
                op,
                worker_tx: worker_tx.clone(),
            })
            .map_err(|_| anyhow!("Clipboard worker thread is not running"))?;
        Ok(pending)
    }

    #[cfg(test)]
    pub(crate) fn from_fn(copy_text_fn: fn(&str) -> Result<()>) -> Self {
        let (tx, rx) = mpsc::channel::<CopyRequest>();

        std::thread::Builder::new()
            .name("clipboard-test".into())
            .spawn(move || {
                while let Ok(req) = rx.recv() {
                    let result = (copy_text_fn)(&req.text).map_err(|e| e.to_string());
                    let status = req.op.resolve(&result);
                    let _ = req.worker_tx.send(WorkerEvent::ClipboardCopyCompleted {
                        label: req.label,
                        result,
                        status,
                    });
                }
            })
            .expect("failed to spawn test clipboard thread");

        Self { tx }
    }
}

/// Maximum payload size for an OSC 52 copy. Many terminals refuse longer
/// sequences. 100 KiB matches the limit used by tmux and WezTerm.
const OSC52_MAX_BYTES: usize = 100_000;

/// True when no X11 or Wayland display is reachable, so arboard's Linux/BSD
/// backends would hang or time out ("X11 server connection timed out").
/// macOS and Windows clipboards need no display variable, so this is never
/// true there.
fn no_display() -> bool {
    if cfg!(all(unix, not(target_os = "macos"), not(target_os = "ios"))) {
        std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none()
    } else {
        false
    }
}

/// Minimal RFC 4648 base64 encoder so the fallback needs no extra dependency.
fn base64_encode(data: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).copied().map_or(0, u32::from);
        let b2 = chunk.get(2).copied().map_or(0, u32::from);
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(A[((n >> 18) & 0x3f) as usize] as char);
        out.push(A[((n >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            A[((n >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            A[(n & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Format an OSC 52 escape sequence targeting the system clipboard ("c").
/// BEL is used as the terminator because it is more widely supported than ST.
fn osc52_sequence(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64_encode(text.as_bytes()))
}

/// Emit an OSC 52 copy via /dev/tty so the escape reaches the controlling
/// terminal regardless of stdout/stderr redirection. The host terminal
/// (WezTerm, kitty, alacritty, the VS Code terminal, tmux with
/// `set-clipboard on`, and so on) writes the payload to the local clipboard.
/// This is the standard way to copy from a remote shell with no X server.
fn osc52_copy(text: &str) -> Result<()> {
    use std::io::Write;
    if text.len() > OSC52_MAX_BYTES {
        return Err(anyhow!(
            "text too large for OSC 52 clipboard ({} bytes; cap is {OSC52_MAX_BYTES})",
            text.len()
        ));
    }
    let mut tty = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/tty")
        .map_err(|e| anyhow!("cannot open /dev/tty for OSC 52: {e}"))?;
    tty.write_all(osc52_sequence(text).as_bytes())
        .map_err(|e| anyhow!("OSC 52 write failed: {e}"))?;
    let _ = tty.flush();
    Ok(())
}

fn clipboard_worker(rx: mpsc::Receiver<CopyRequest>) {
    // Without a display server arboard's X11 path hangs until it times out,
    // so skip it and go straight to OSC 52 (headless SSH, VS Code Remote).
    let primary: Result<arboard::Clipboard, String> = if no_display() {
        Err("no display server".to_string())
    } else {
        arboard::Clipboard::new().map_err(|e| e.to_string())
    };
    run_worker(
        rx,
        primary.map(|mut board| move |text: &str| board.set_text(text).map_err(|e| e.to_string())),
        osc52_copy,
    );
}

/// Drain copy requests. `primary` is the native clipboard setter (or the
/// reason it is unavailable). When it is unavailable, every request goes to
/// `fallback` (OSC 52). When it is available but one `set` fails (server
/// timeout, lost selection ownership), that request is retried through
/// `fallback` before the failure is reported.
fn run_worker<P>(
    rx: mpsc::Receiver<CopyRequest>,
    primary: Result<P, String>,
    fallback: impl Fn(&str) -> Result<()>,
) where
    P: FnMut(&str) -> std::result::Result<(), String>,
{
    let mut primary = primary.ok();
    while let Ok(req) = rx.recv() {
        let result = match primary.as_mut().map(|set| set(&req.text)) {
            Some(Ok(())) => Ok(()),
            Some(Err(_)) | None => {
                fallback(&req.text).map_err(|e| format!("Failed to copy to clipboard: {e}"))
            }
        };
        let status = req.op.resolve(&result);
        let _ = req.worker_tx.send(WorkerEvent::ClipboardCopyCompleted {
            label: req.label,
            result,
            status,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_from_fn_sends_and_receives() {
        let (worker_tx, worker_rx) = mpsc::channel();
        let clipboard = Clipboard::from_fn(|_| Ok(()));
        clipboard.copy_text("hello", "Copied.", &worker_tx).unwrap();

        let event = worker_rx.recv().unwrap();
        match event {
            WorkerEvent::ClipboardCopyCompleted {
                label,
                result,
                status,
            } => {
                assert_eq!(label, "Copied.");
                assert!(result.is_ok());
                assert_eq!(status.outcome, Final::info("Copied."));
            }
            _ => panic!("unexpected event"),
        }
    }

    #[test]
    fn clipboard_from_fn_reports_errors() {
        let (worker_tx, worker_rx) = mpsc::channel();
        let clipboard = Clipboard::from_fn(|_| Err(anyhow!("test error")));
        clipboard.copy_text("hello", "Copied.", &worker_tx).unwrap();

        let event = worker_rx.recv().unwrap();
        match event {
            WorkerEvent::ClipboardCopyCompleted {
                label,
                result,
                status,
            } => {
                assert_eq!(label, "Copied.");
                assert!(result.unwrap_err().contains("test error"));
                assert!(matches!(
                    status.outcome,
                    Final::Message {
                        tone: dux_core::statusline::StatusTone::Error,
                        ..
                    }
                ));
            }
            _ => panic!("unexpected event"),
        }
    }

    fn request(
        text: &str,
        worker_tx: &mpsc::Sender<WorkerEvent>,
    ) -> (mpsc::Sender<CopyRequest>, mpsc::Receiver<CopyRequest>) {
        let (tx, rx) = mpsc::channel();
        let op = status_op("copy")
            .on_success(|_: &()| Final::info("ok"))
            .on_failure(|e: &String| Final::error(e.clone()));
        tx.send(CopyRequest {
            text: text.to_string(),
            label: "Copied.".to_string(),
            op,
            worker_tx: worker_tx.clone(),
        })
        .unwrap();
        (tx, rx)
    }

    fn run_one<P>(
        primary: Result<P, String>,
        fallback_ok: bool,
    ) -> (Result<(), String>, Vec<String>)
    where
        P: FnMut(&str) -> std::result::Result<(), String>,
    {
        let (worker_tx, worker_rx) = mpsc::channel();
        let (tx, rx) = request("payload", &worker_tx);
        drop(tx);
        let seen = std::cell::RefCell::new(Vec::new());
        run_worker(rx, primary, |t: &str| {
            seen.borrow_mut().push(t.to_string());
            if fallback_ok {
                Ok(())
            } else {
                Err(anyhow!("no tty"))
            }
        });
        let WorkerEvent::ClipboardCopyCompleted { result, .. } = worker_rx.recv().unwrap() else {
            panic!("unexpected event");
        };
        (result, seen.into_inner())
    }

    type Setter = fn(&str) -> std::result::Result<(), String>;

    #[test]
    fn unavailable_native_clipboard_falls_back_to_osc52() {
        // The fork's failure mode: arboard cannot reach an X server over SSH.
        let (result, seen) = run_one::<Setter>(Err("X11 server connection timed out".into()), true);
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(seen, vec!["payload".to_string()]);
    }

    #[test]
    fn failed_native_set_retries_via_osc52() {
        let (result, seen) = run_one::<Setter>(Ok(|_| Err("lost ownership".into())), true);
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(seen, vec!["payload".to_string()]);
    }

    #[test]
    fn successful_native_set_does_not_emit_osc52() {
        let (result, seen) = run_one::<Setter>(Ok(|_| Ok(())), true);
        assert!(result.is_ok());
        assert!(seen.is_empty());
    }

    #[test]
    fn fallback_failure_is_reported() {
        let (result, _) = run_one::<Setter>(Err("no display server".into()), false);
        assert!(result.unwrap_err().contains("no tty"));
    }

    #[test]
    fn base64_known_vectors() {
        // RFC 4648 section 10 test vectors.
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn osc52_sequence_format() {
        assert_eq!(osc52_sequence("hi"), "\x1b]52;c;aGk=\x07");
    }

    #[test]
    fn osc52_rejects_oversized_payload() {
        let big = "x".repeat(OSC52_MAX_BYTES + 1);
        assert!(
            osc52_copy(&big)
                .unwrap_err()
                .to_string()
                .contains("too large")
        );
    }
}

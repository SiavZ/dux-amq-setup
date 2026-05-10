use std::backtrace::Backtrace;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::panic;
use std::path::PathBuf;

/// Install a panic hook that leaves a durable breadcrumb outside the structured
/// tracing pipeline. This catches panics that happen before tracing flushes and
/// gives operators a backtrace when `dux.log` simply stops.
pub(crate) fn install_panic_hook(root: PathBuf) {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
            .unwrap_or("<non-string panic payload>");
        let location = info
            .location()
            .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()))
            .unwrap_or_else(|| "<unknown location>".to_string());
        let backtrace = Backtrace::force_capture();
        let body = format!(
            "\n=== dux panic ===\ntime: {}\nlocation: {location}\npayload: {payload}\nbacktrace:\n{backtrace}\n",
            chrono::Utc::now().to_rfc3339(),
        );
        append_crash_log(&root, &body);
        previous(info);
    }));
}

fn append_crash_log(root: &PathBuf, body: &str) {
    if let Err(err) = fs::create_dir_all(root) {
        eprintln!(
            "dux: failed to create crash log dir {}: {err}",
            root.display()
        );
        return;
    }
    let path = root.join("dux-crash.log");
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut file) => {
            let _ = file.write_all(body.as_bytes());
            let _ = file.flush();
        }
        Err(err) => {
            eprintln!("dux: failed to write crash log {}: {err}", path.display());
        }
    }
}

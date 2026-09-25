//! Filesystem probes for AMQ collaboration state.
//!
//! Dux intentionally does not own AMQ's mailbox format, but it can make
//! conservative decisions from the stable Maildir-like layout used by the
//! CLI: unread messages in `inbox/new`, drafts in `outbox/pending`, and
//! recent message/receipt file mtimes.

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

const AMQ_ACTIVITY_DIRS: &[&str] = &[
    "inbox/new",
    "inbox/cur",
    "outbox/pending",
    "outbox/sent",
    "receipts",
];

/// Returns true when the agent has unread incoming messages or unsent
/// outgoing drafts. This is a hard blocker for auto-clear.
pub fn has_pending_mail(agent_dir: &Path) -> bool {
    has_any_file(&agent_dir.join("inbox/new")) || has_any_file(&agent_dir.join("outbox/pending"))
}

/// Returns true when the agent has sent, received, drained, or receipt
/// activity within `quiet_for`.
pub fn has_recent_activity(agent_dir: &Path, quiet_for: Duration, now: SystemTime) -> bool {
    if quiet_for.is_zero() {
        return false;
    }
    AMQ_ACTIVITY_DIRS
        .iter()
        .any(|rel| dir_has_recent_file(&agent_dir.join(rel), quiet_for, now).unwrap_or(false))
}

/// Returns true when Dux's bridge/drainer queue still has a pending wake
/// for this receiver.
pub fn has_pending_inject(queue_root: &Path, receiver: &str) -> bool {
    let dir = queue_root.join(receiver);
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            return false;
        };
        name.ends_with(".msg") || (name.starts_with(".inflight.") && name.ends_with(".msg"))
    })
}

fn has_any_file(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries
        .filter_map(Result::ok)
        .any(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
}

fn dir_has_recent_file(dir: &Path, quiet_for: Duration, now: SystemTime) -> std::io::Result<bool> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let modified = entry.metadata()?.modified()?;
        if now.duration_since(modified).unwrap_or_default() <= quiet_for {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use filetime::{FileTime, set_file_mtime};

    #[test]
    fn pending_mail_detects_unread_and_outbox_drafts() {
        let tmp = tempfile::tempdir().unwrap();
        let agent = tmp.path().join("agents/alice");
        fs::create_dir_all(agent.join("inbox/new")).unwrap();
        assert!(!has_pending_mail(&agent));

        fs::write(agent.join("inbox/new/msg.md"), "hello").unwrap();
        assert!(has_pending_mail(&agent));

        fs::remove_file(agent.join("inbox/new/msg.md")).unwrap();
        fs::create_dir_all(agent.join("outbox/pending")).unwrap();
        fs::write(agent.join("outbox/pending/draft.md"), "hello").unwrap();
        assert!(has_pending_mail(&agent));
    }

    #[test]
    fn recent_activity_respects_quiet_window() {
        let tmp = tempfile::tempdir().unwrap();
        let agent = tmp.path().join("agents/alice");
        let cur = agent.join("inbox/cur");
        fs::create_dir_all(&cur).unwrap();
        let file = cur.join("msg.md");
        fs::write(&file, "hello").unwrap();

        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        set_file_mtime(&file, FileTime::from_unix_time(990, 0)).unwrap();
        assert!(has_recent_activity(&agent, Duration::from_secs(30), now));

        set_file_mtime(&file, FileTime::from_unix_time(900, 0)).unwrap();
        assert!(!has_recent_activity(&agent, Duration::from_secs(30), now));
        assert!(!has_recent_activity(&agent, Duration::ZERO, now));
    }

    #[test]
    fn pending_inject_detects_plain_and_inflight_messages_only() {
        let tmp = tempfile::tempdir().unwrap();
        let receiver = tmp.path().join("bob");
        fs::create_dir_all(&receiver).unwrap();
        assert!(!has_pending_inject(tmp.path(), "bob"));

        fs::write(receiver.join("queued.msg"), "hello").unwrap();
        assert!(has_pending_inject(tmp.path(), "bob"));

        fs::remove_file(receiver.join("queued.msg")).unwrap();
        fs::write(receiver.join(".inflight.queued.msg"), "hello").unwrap();
        assert!(has_pending_inject(tmp.path(), "bob"));

        fs::remove_file(receiver.join(".inflight.queued.msg")).unwrap();
        fs::write(receiver.join("tmpfile"), "hello").unwrap();
        assert!(!has_pending_inject(tmp.path(), "bob"));
    }
}

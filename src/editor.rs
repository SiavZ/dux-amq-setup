use std::collections::HashSet;
use std::env;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorKind {
    Cursor,
    VsCode,
    Zed,
    Antigravity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetectedEditor {
    pub kind: EditorKind,
    pub label: &'static str,
    pub config_key: &'static str,
    pub command: String,
}

struct EditorSpec {
    kind: EditorKind,
    label: &'static str,
    config_key: &'static str,
    commands: &'static [&'static str],
    aliases: &'static [&'static str],
}

const EDITOR_SPECS: &[EditorSpec] = &[
    EditorSpec {
        kind: EditorKind::Cursor,
        label: "Cursor",
        config_key: "cursor",
        commands: &["cursor"],
        aliases: &["cursor"],
    },
    EditorSpec {
        kind: EditorKind::VsCode,
        label: "VS Code",
        config_key: "vscode",
        commands: &["code", "code-insiders"],
        aliases: &["vscode", "code", "code-insiders"],
    },
    EditorSpec {
        kind: EditorKind::Zed,
        label: "Zed",
        config_key: "zed",
        commands: &["zed"],
        aliases: &["zed"],
    },
    EditorSpec {
        kind: EditorKind::Antigravity,
        label: "Antigravity",
        config_key: "antigravity",
        commands: &["antigravity"],
        aliases: &["antigravity"],
    },
];

pub fn detect_installed_editors() -> Vec<DetectedEditor> {
    let mut executable_names = Vec::new();
    let mut seen_dirs = HashSet::new();
    let path = env::var_os("PATH").unwrap_or_default();

    for dir in env::split_paths(&path) {
        if !seen_dirs.insert(dir.clone()) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                executable_names.push(name.to_string());
            }
        }
    }

    detect_editors_from_names(executable_names.iter().map(String::as_str))
}

pub fn preferred_editor(detected: &[DetectedEditor], configured: &str) -> Option<DetectedEditor> {
    let configured = normalize_editor_name(configured);
    if !configured.is_empty()
        && let Some(editor) = detected
            .iter()
            .find(|editor| matches_configured_editor(editor, &configured))
    {
        return Some(editor.clone());
    }
    detected.first().cloned()
}

pub fn matches_configured_editor(editor: &DetectedEditor, configured: &str) -> bool {
    let configured = normalize_editor_name(configured);
    if configured.is_empty() {
        return false;
    }
    let spec = spec_for_kind(editor.kind);
    configured == spec.config_key
        || spec.aliases.iter().any(|alias| *alias == configured)
        || spec.commands.iter().any(|command| *command == configured)
}

pub fn launch_editor(editor: &DetectedEditor, path: &Path) -> Result<()> {
    if !path.exists() {
        return Err(anyhow!("Path does not exist: {}", path.display()));
    }

    Command::new(&editor.command)
        .args(editor_launch_args(editor, path))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to launch {} via {}", editor.label, editor.command))?;

    Ok(())
}

fn editor_launch_args(editor: &DetectedEditor, path: &Path) -> Vec<OsString> {
    let mut args = Vec::new();
    match editor.kind {
        EditorKind::Cursor | EditorKind::VsCode | EditorKind::Antigravity => {
            args.push(OsString::from("--new-window"));
        }
        EditorKind::Zed => {}
    }
    args.push(path.as_os_str().to_os_string());
    args
}

fn detect_editors_from_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<DetectedEditor> {
    let normalized_names = names
        .into_iter()
        .map(normalize_executable_name)
        .collect::<HashSet<_>>();

    EDITOR_SPECS
        .iter()
        .filter_map(|spec| {
            spec.commands.iter().find_map(|command| {
                normalized_names.contains(*command).then(|| DetectedEditor {
                    kind: spec.kind,
                    label: spec.label,
                    config_key: spec.config_key,
                    command: (*command).to_string(),
                })
            })
        })
        .collect()
}

fn normalize_editor_name(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace(['_', ' '], "-")
}

fn normalize_executable_name(value: &str) -> String {
    Path::new(value)
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or(value)
        .to_ascii_lowercase()
}

fn spec_for_kind(kind: EditorKind) -> &'static EditorSpec {
    EDITOR_SPECS
        .iter()
        .find(|spec| spec.kind == kind)
        .expect("editor kind should always have a matching spec")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn detect_editors_prefers_popular_order() {
        let detected = detect_editors_from_names(["zed", "cursor", "code"].iter().copied());
        let labels = detected
            .iter()
            .map(|editor| editor.label)
            .collect::<Vec<_>>();
        assert_eq!(labels, vec!["Cursor", "VS Code", "Zed"]);
    }

    #[test]
    fn preferred_editor_uses_configured_alias_when_available() {
        let detected = detect_editors_from_names(["code", "cursor"].iter().copied());
        let preferred = preferred_editor(&detected, "vscode").expect("editor should resolve");
        assert_eq!(preferred.label, "VS Code");
        assert_eq!(preferred.command, "code");
    }

    #[test]
    fn preferred_editor_falls_back_to_first_detected() {
        let detected = detect_editors_from_names(["zed"].iter().copied());
        let preferred = preferred_editor(&detected, "cursor").expect("editor should resolve");
        assert_eq!(preferred.label, "Zed");
    }

    #[test]
    fn configured_matching_accepts_command_aliases() {
        let detected = detect_editors_from_names(["code-insiders"].iter().copied());
        let editor = detected.first().expect("editor should be detected");
        assert!(matches_configured_editor(editor, "vscode"));
        assert!(matches_configured_editor(editor, "code-insiders"));
    }

    #[test]
    fn vscode_launch_args_force_new_window() {
        let editor = DetectedEditor {
            kind: EditorKind::VsCode,
            label: "VS Code",
            config_key: "vscode",
            command: "code".to_string(),
        };
        let path = PathBuf::from("/tmp/agent-worktree");

        assert_eq!(
            editor_launch_args(&editor, &path),
            vec![
                OsString::from("--new-window"),
                OsString::from("/tmp/agent-worktree")
            ]
        );
    }

    #[test]
    fn cursor_launch_args_force_new_window() {
        let editor = DetectedEditor {
            kind: EditorKind::Cursor,
            label: "Cursor",
            config_key: "cursor",
            command: "cursor".to_string(),
        };
        let path = PathBuf::from("/tmp/agent-worktree");

        assert_eq!(
            editor_launch_args(&editor, &path),
            vec![
                OsString::from("--new-window"),
                OsString::from("/tmp/agent-worktree")
            ]
        );
    }

    #[test]
    fn zed_launch_args_keep_single_path_arg() {
        let editor = DetectedEditor {
            kind: EditorKind::Zed,
            label: "Zed",
            config_key: "zed",
            command: "zed".to_string(),
        };
        let path = PathBuf::from("/tmp/agent-worktree");

        assert_eq!(
            editor_launch_args(&editor, &path),
            vec![OsString::from("/tmp/agent-worktree")]
        );
    }
}

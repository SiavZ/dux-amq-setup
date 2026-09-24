//! The dux-amq overlay's config contract, checked against THIS build of dux.
//!
//! The overlay (`dux-amq/`) changes a handful of provider settings in a user's
//! `config.toml`. It does that two ways, and both drift silently when upstream
//! renames a key or changes how it renders a default:
//!
//! - `dux-amq/config/dux-config.sed` is what `install.sh` actually runs. Every
//!   rule matches one exact rendered line, so a changed default makes a rule
//!   match nothing, and sed reports success regardless.
//! - `dux-amq/config/dux-config-changes.toml` is the same end state in config
//!   form. dux ignores unknown keys on load (no `deny_unknown_fields`), so a
//!   renamed key there parses fine and quietly does nothing.
//!
//! These tests close both gaps: the snippet must name only keys the current
//! schema knows, and the sed program, run over a config the real binary just
//! regenerated, must produce exactly the snippet's values.

use std::path::{Path, PathBuf};
use std::process::Command;

use dux_core::config::{Config, DuxPaths, load_config};

fn overlay_file(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../dux-amq/config")
        .join(name)
}

fn snippet() -> toml::Table {
    let raw = std::fs::read_to_string(overlay_file("dux-config-changes.toml"))
        .expect("read dux-config-changes.toml");
    raw.parse::<toml::Table>()
        .expect("dux-config-changes.toml is valid TOML")
}

/// Every leaf key path in `table`, as dotted strings, paired with its value.
fn leaves(prefix: &str, table: &toml::Table, out: &mut Vec<(String, toml::Value)>) {
    for (key, value) in table {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            toml::Value::Table(inner) => leaves(&path, inner, out),
            other => out.push((path, other.clone())),
        }
    }
}

fn lookup<'a>(table: &'a toml::Table, path: &str) -> Option<&'a toml::Value> {
    let mut parts = path.split('.');
    let mut current = table.get(parts.next()?)?;
    for part in parts {
        current = current.as_table()?.get(part)?;
    }
    Some(current)
}

/// Keys in `snippet` that `Config` does not round-trip, or round-trips to a
/// different value. A key serde ignored (unknown to the schema) is absent from
/// the re-serialized config, so this is a strict unknown-key check even though
/// the loader itself is lenient.
fn schema_mismatches(snippet: &toml::Table) -> Vec<String> {
    let parsed: Config = toml::from_str(&toml::to_string(snippet).expect("reserialize snippet"))
        .expect("the snippet must deserialize into dux's Config");
    let roundtrip: toml::Table =
        toml::from_str(&toml::to_string(&parsed).expect("serialize Config")).expect("reparse");
    let mut leaf_list = Vec::new();
    leaves("", snippet, &mut leaf_list);
    leaf_list
        .into_iter()
        .filter_map(|(path, value)| match lookup(&roundtrip, &path) {
            Some(got) if *got == value => None,
            Some(got) => Some(format!("{path}: snippet says {value}, Config holds {got}")),
            None => Some(format!("{path}: not a key the config schema knows")),
        })
        .collect()
}

#[test]
fn the_overlay_config_snippet_uses_only_keys_the_schema_knows() {
    let mismatches = schema_mismatches(&snippet());
    assert!(
        mismatches.is_empty(),
        "dux-config-changes.toml has drifted from the config schema:\n  {}",
        mismatches.join("\n  ")
    );
}

/// Guard for the check above: without it, a lenient comparison could pass
/// every snippet. A retired key (the fork's `prompt_for_name`) and a key the
/// schema has not grown yet must both be reported.
#[test]
fn the_schema_check_rejects_unknown_and_retired_keys() {
    let bad: toml::Table = toml::from_str(
        "[defaults]\nprompt_for_name = true\n\n[providers.claude]\ncommand = \"claude-amq\"\nforward_mouse = false\nforward_mous = false\n",
    )
    .expect("parse");
    let mismatches = schema_mismatches(&bad);
    assert!(
        mismatches
            .iter()
            .any(|m| m.starts_with("defaults.prompt_for_name")),
        "{mismatches:?}"
    );
    // A misspelling is exactly what the lenient loader would swallow.
    assert!(
        mismatches
            .iter()
            .any(|m| m.starts_with("providers.claude.forward_mous:")),
        "{mismatches:?}"
    );
    assert!(
        !mismatches
            .iter()
            .any(|m| m.starts_with("providers.claude.command")
                || m.starts_with("providers.claude.forward_mouse:")),
        "a known key with the right value must not be reported: {mismatches:?}"
    );
}

fn paths_in(root: &Path) -> DuxPaths {
    DuxPaths {
        root: root.to_path_buf(),
        config_path: root.join("config.toml"),
        sessions_db_path: root.join("sessions.sqlite3"),
        worktrees_root: root.join("worktrees"),
        lock_path: root.join("dux.lock"),
    }
}

/// Regenerate a stock config with the real binary, exactly as `install.sh`
/// does on a fresh machine.
fn regenerate_stock_config(root: &Path) -> String {
    let status = Command::new(env!("CARGO_BIN_EXE_dux"))
        .args(["config", "regenerate", "--yes"])
        .env("DUX_HOME", root)
        .env("HOME", root)
        .status()
        .expect("run dux config regenerate");
    assert!(status.success(), "dux config regenerate --yes failed");
    std::fs::read_to_string(root.join("config.toml")).expect("read regenerated config")
}

/// Run the overlay's sed program over `input`, the way install.sh does
/// (`sed -f`, to stdout here instead of `-i`).
fn run_overlay_sed(dir: &Path, input: &str) -> String {
    let file = dir.join("sed-input.toml");
    std::fs::write(&file, input).expect("write sed input");
    let out = Command::new("sed")
        .arg("-f")
        .arg(overlay_file("dux-config.sed"))
        .arg(&file)
        .output()
        .expect("run sed");
    assert!(
        out.status.success(),
        "sed rejected dux-config.sed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("sed output is UTF-8")
}

fn load(root: &Path, text: &str) -> Config {
    let paths = paths_in(root);
    std::fs::write(&paths.config_path, text).expect("write patched config");
    load_config(&paths)
}

fn assert_snippet_holds(config: &Config, what: &str) {
    let snippet = snippet();
    let loaded: toml::Table =
        toml::from_str(&toml::to_string(config).expect("serialize")).expect("reparse");
    let mut leaf_list = Vec::new();
    leaves("", &snippet, &mut leaf_list);
    let wrong: Vec<String> = leaf_list
        .into_iter()
        .filter(|(path, value)| lookup(&loaded, path) != Some(value))
        .map(|(path, value)| {
            format!(
                "{path}: want {value}, {what} has {:?}",
                lookup(&loaded, &path).map(ToString::to_string)
            )
        })
        .collect();
    assert!(
        wrong.is_empty(),
        "dux-config.sed no longer produces what dux-config-changes.toml documents \
         (a rule has stopped matching the rendered config, or the two files disagree):\n  {}",
        wrong.join("\n  ")
    );
}

#[test]
fn the_installer_sed_turns_a_fresh_config_into_the_documented_end_state() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let stock = regenerate_stock_config(tmp.path());
    let patched = run_overlay_sed(tmp.path(), &stock);
    assert_ne!(patched, stock, "the overlay changed nothing at all");
    let config = load(tmp.path(), &patched);
    assert_snippet_holds(&config, "the patched config");

    // The patch must not cost the user the documentation comments dux wrote:
    // only the targeted lines change.
    assert_eq!(
        stock.lines().count() + 1,
        patched.lines().count(),
        "exactly one line (codex's resume_by_id_args) is added; every other rule rewrites in place"
    );
}

/// install.sh runs the program on EVERY rerun, over a config the first run
/// already patched (and that the user may have edited since). A second pass
/// must be byte-for-byte a no-op, or reruns would stack appended lines.
#[test]
fn the_installer_sed_is_a_no_op_on_an_already_patched_config() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let stock = regenerate_stock_config(tmp.path());
    let once = run_overlay_sed(tmp.path(), &stock);
    let twice = run_overlay_sed(tmp.path(), &once);
    assert_eq!(once, twice);
}

/// gemini is no longer a stock provider, so a fresh config never exercises
/// its rules. A block the user kept must still be routed through the wrapper.
#[test]
fn the_installer_sed_wraps_a_gemini_block_the_user_kept() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut stock = regenerate_stock_config(tmp.path());
    stock.push_str(
        "\n[providers.gemini]\ncommand = \"gemini\"\nargs = []\nforward_scroll = false\n",
    );
    let patched = run_overlay_sed(tmp.path(), &stock);
    let config = load(tmp.path(), &patched);
    let gemini = &config.providers.commands["gemini"];
    assert_eq!(gemini.command, "gemini-amq");
    assert_eq!(gemini.forward_scroll, Some(true));
}

/// Current dux already renders `forward_mouse = false` for claude and codex,
/// so a fresh config never exercises the overlay's forward_mouse rules. A
/// config the user edited can carry an explicit `true`; the overlay must still
/// pin it off so plain drags stay in dux and pane text can be copied
/// (bc3a9eec). (A commented-out or absent key needs no rule: the loader fills
/// it with the shipped `false`. The codex leg checks that stays true.)
#[test]
fn the_installer_sed_pins_forward_mouse_off_in_a_kept_config() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let stock = regenerate_stock_config(tmp.path());
    let mut section = "";
    let kept: String = stock
        .lines()
        .map(|line| {
            if line.starts_with('[') {
                section = line;
            }
            match (section, line) {
                ("[providers.claude]", "forward_mouse = false") => "forward_mouse = true",
                ("[providers.codex]", "forward_mouse = false") => "# forward_mouse = true",
                _ => line,
            }
        })
        .map(|line| format!("{line}\n"))
        .collect();
    assert_ne!(kept, stock, "the fixture must actually flip forward_mouse");
    let patched = run_overlay_sed(tmp.path(), &kept);
    let config = load(tmp.path(), &patched);
    for name in ["claude", "codex"] {
        assert_eq!(
            config.providers.commands[name].forward_mouse,
            Some(false),
            "{name}: forward_mouse is not pinned off"
        );
    }
}

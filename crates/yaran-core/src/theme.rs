//! Surface-agnostic theme identity.
//!
//! The full theme model (the ratatui `Theme` struct, opaline-backed color
//! loading, and `Style`/`Span` helpers) is rendering-specific and lives in the
//! TUI surface (its loaders also depend on config/logger). Only theme identity
//! that domain/config code needs belongs here.

/// Name of the bundled default theme, also the value written into the
/// generated `config.toml` on first boot.
pub const DEFAULT_THEME_NAME: &str = "yaran_dark";

#[cfg(test)]
mod tests {
    #[test]
    fn legacy_bundled_theme_names_pass_core_config_validation() {
        for name in ["dux_dark", "dux-dark", "yaran-dark", "yaran_dark"] {
            let config =
                crate::config::validate_config_str(&format!("[ui]\ntheme = {name:?}\n")).unwrap();
            assert_eq!(config.ui.theme, name);
        }
    }

    #[test]
    fn custom_theme_names_are_not_rewritten() {
        let config = crate::config::validate_config_str("[ui]\ntheme = 'my-theme'\n").unwrap();
        assert_eq!(config.ui.theme, "my-theme");
    }

    #[test]
    fn loading_a_legacy_theme_keeps_the_user_config_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let paths = crate::config::YaranPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root,
        };
        for alias in ["dux_dark", "dux-dark"] {
            let raw = format!("# Keep my comments\n[ui]\ntheme = {alias:?}\n");
            std::fs::write(&paths.config_path, &raw).unwrap();
            let config = crate::config::load_config(&paths);
            assert_eq!(config.ui.theme, alias);
            assert_eq!(std::fs::read_to_string(&paths.config_path).unwrap(), raw);
        }
    }
}

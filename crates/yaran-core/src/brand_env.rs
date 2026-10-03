//! Read Yaran settings without breaking environments configured for dux.
//!
//! A present YARAN setting always wins, including an empty value. Only a missing
//! new setting consults the corresponding legacy DUX setting.

use std::env::VarError;
use std::ffi::OsString;

fn legacy_name(name: &str) -> Option<String> {
    name.strip_prefix("YARAN_")
        .map(|suffix| format!("DUX_{suffix}"))
}

pub fn var(name: &str) -> Result<String, VarError> {
    match std::env::var(name) {
        Err(VarError::NotPresent) => match legacy_name(name) {
            Some(legacy) => std::env::var(legacy),
            None => Err(VarError::NotPresent),
        },
        result => result,
    }
}

pub fn var_os(name: &str) -> Option<OsString> {
    std::env::var_os(name).or_else(|| legacy_name(name).and_then(std::env::var_os))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_settings_win_even_when_empty_and_legacy_settings_remain_readable() {
        let _guard = crate::env_test_guard();
        let new = "YARAN_REBRAND_ENV_TEST";
        let old = "DUX_REBRAND_ENV_TEST";
        // These dedicated names have no production meaning. The guard serializes
        // all tests that mutate this process's environment.
        unsafe {
            std::env::remove_var(new);
            std::env::set_var(old, "legacy");
        }
        assert_eq!(var(new).unwrap(), "legacy");
        assert_eq!(var_os(new).unwrap(), "legacy");
        unsafe { std::env::set_var(new, "new") };
        assert_eq!(var(new).unwrap(), "new");
        assert_eq!(var_os(new).unwrap(), "new");
        unsafe { std::env::set_var(new, "") };
        assert_eq!(var(new).unwrap(), "");
        assert_eq!(var_os(new).unwrap(), "");
        unsafe {
            std::env::remove_var(new);
            std::env::remove_var(old);
        }
        assert!(matches!(var(new), Err(VarError::NotPresent)));
        assert!(var_os(new).is_none());
    }

    #[test]
    fn unrelated_names_do_not_get_a_legacy_alias() {
        assert_eq!(legacy_name("YARAN_HOME"), Some("DUX_HOME".into()));
        assert_eq!(legacy_name("XDG_CONFIG_HOME"), None);
        assert_eq!(legacy_name("HOME"), None);
    }
}

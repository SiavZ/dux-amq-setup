//! Compatibility for build inputs produced by dux before the Yaran rebrand.

use std::ffi::OsStr;

pub fn skip_ui_build(primary: Option<&OsStr>, legacy: Option<&OsStr>) -> bool {
    primary.or(legacy).is_some_and(|value| !value.is_empty())
}

pub fn is_notice_page(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    text.contains("yaran-ui-not-built-notice") || text.contains("dux-ui-not-built-notice")
}

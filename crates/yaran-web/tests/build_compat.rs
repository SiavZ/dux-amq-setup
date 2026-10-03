#[path = "../build_compat.rs"]
mod build_compat;

use std::ffi::OsStr;

#[test]
fn new_build_flag_wins_and_the_old_flag_still_works() {
    let empty = Some(OsStr::new(""));
    let enabled = Some(OsStr::new("1"));
    assert!(!build_compat::skip_ui_build(None, None));
    assert!(!build_compat::skip_ui_build(None, empty));
    assert!(build_compat::skip_ui_build(None, enabled));
    assert!(build_compat::skip_ui_build(enabled, None));
    assert!(build_compat::skip_ui_build(enabled, empty));
    assert!(!build_compat::skip_ui_build(empty, enabled));
}

#[test]
fn notice_detection_accepts_both_brands_but_not_a_real_frontend() {
    for brand in ["yaran", "dux"] {
        let notice = format!("<!doctype html><!-- {brand}-ui-not-built-notice -->");
        assert!(build_compat::is_notice_page(notice.as_bytes()));
    }
    assert!(!build_compat::is_notice_page(
        b"<!doctype html><html><head><title>Yaran</title></head><div id=\"root\"></div>"
    ));
}

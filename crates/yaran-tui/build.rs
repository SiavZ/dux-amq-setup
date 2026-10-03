use std::env;

fn main() {
    println!("cargo:rerun-if-env-changed=YARAN_RELEASE_BUILD");
    println!("cargo:rerun-if-env-changed=DUX_RELEASE_BUILD");

    let release_build =
        env::var_os("YARAN_RELEASE_BUILD").or_else(|| env::var_os("DUX_RELEASE_BUILD"));
    let display_version = if release_build.as_deref() == Some(std::ffi::OsStr::new("1")) {
        format!("v{}", env!("CARGO_PKG_VERSION"))
    } else {
        "development".to_string()
    };

    println!("cargo:rustc-env=YARAN_DISPLAY_VERSION={display_version}");
}

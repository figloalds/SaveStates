// Raven v0.2.0's static library also needs WinHTTP (rvn_fs_download_url), which the vendored
// binding's build.rs does not list. Raven's own CMake package lists it
// (lib/cmake/raven/ravenTargets.cmake: winhttp;user32;gdi32;shell32;comdlg32;ole32).
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("RAVEN_STATIC").as_deref() == Ok("1")
    {
        println!("cargo:rustc-link-lib=winhttp");
    }
    println!("cargo:rerun-if-env-changed=RAVEN_STATIC");
}

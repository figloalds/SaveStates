//! Embeds the app icon and the version information into savestates.exe (Windows only).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/savestates.ico");
    // The check is on the target, not the machine the build script runs on.
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resources = winresource::WindowsResource::new();
        resources.set_icon("../../assets/savestates.ico");
        if let Err(error) = resources.compile() {
            panic!("could not embed the icon (needs rc.exe from the Windows SDK): {error}");
        }
    }
}

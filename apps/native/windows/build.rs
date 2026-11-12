//! Embeds the Captures icon and name in the executable: Task Manager and
//! Explorer show FileDescription, not the window title or the file name.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../desktop/src-tauri/icons/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("../../desktop/src-tauri/icons/icon.ico");
        resource.set("FileDescription", "Captures");
        resource.set("ProductName", "Captures");
        if let Err(error) = resource.compile() {
            panic!("embed the Windows executable resources: {error}");
        }
    }
}

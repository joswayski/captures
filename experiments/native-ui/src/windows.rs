#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

// The Windows app shares the full native widget tree, Cairo rendering and
// animations with Linux. Platform integration is cfg-gated in native/windows.rs.
#[cfg(target_os = "windows")]
include!("native.rs");

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("Build captures-windows-native for Windows; use captures-linux-native on Linux.");
    std::process::exit(1);
}

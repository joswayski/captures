#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // System Monitor and top show the process name, not the binary's.
    #[cfg(target_os = "linux")]
    let _ = std::fs::write("/proc/self/comm", "Captures");
    captures_desktop_lib::run();
}

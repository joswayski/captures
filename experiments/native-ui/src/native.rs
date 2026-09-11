#[cfg(any(target_os = "linux", target_os = "windows"))]
#[path = "native/app.rs"]
mod app;
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[path = "native/capture.rs"]
mod capture;
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[path = "native/desktop.rs"]
mod desktop;
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[path = "native/editor.rs"]
mod editor;
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[path = "native/history.rs"]
mod history;
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[path = "native/preferences.rs"]
mod preferences;
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[path = "native/preview.rs"]
mod preview;
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[path = "native/recording.rs"]
mod recording;
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[path = "native/recovery.rs"]
mod recovery;
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[path = "native/settings.rs"]
mod settings;
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[path = "native/ui.rs"]
mod ui;

#[cfg(target_os = "windows")]
#[path = "native/windows.rs"]
mod windows;

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn main() {
    app::run();
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn main() {
    eprintln!(
        "The native experiment supports Linux and Windows. Use the shipping Tauri app on other platforms."
    );
    std::process::exit(1);
}

#[cfg(target_os = "linux")]
#[path = "native/app.rs"]
mod app;
#[cfg(target_os = "linux")]
#[path = "native/capture.rs"]
mod capture;
#[cfg(target_os = "linux")]
#[path = "native/desktop.rs"]
mod desktop;
#[cfg(target_os = "linux")]
#[path = "native/editor.rs"]
mod editor;
#[cfg(target_os = "linux")]
#[path = "native/history.rs"]
mod history;
#[cfg(target_os = "linux")]
#[path = "native/preferences.rs"]
mod preferences;
#[cfg(target_os = "linux")]
#[path = "native/preview.rs"]
mod preview;
#[cfg(target_os = "linux")]
#[path = "native/recording.rs"]
mod recording;
#[cfg(target_os = "linux")]
#[path = "native/recovery.rs"]
mod recovery;
#[cfg(target_os = "linux")]
#[path = "native/settings.rs"]
mod settings;
#[cfg(target_os = "linux")]
#[path = "native/ui.rs"]
mod ui;

#[cfg(target_os = "linux")]
fn main() {
    app::run();
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("The native frontend is Linux-only. Use the shipping Tauri app on other platforms.");
    std::process::exit(1);
}

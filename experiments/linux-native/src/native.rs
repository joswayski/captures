#[path = "native/app.rs"]
mod app;
#[path = "native/capture.rs"]
mod capture;
#[path = "native/compat.rs"]
mod compat;
#[path = "native/desktop.rs"]
mod desktop;
#[path = "native/editor.rs"]
mod editor;
#[path = "native/history.rs"]
mod history;
#[path = "native/preferences.rs"]
mod preferences;
#[path = "native/preview.rs"]
mod preview;
#[path = "native/recording.rs"]
mod recording;
#[path = "native/recovery.rs"]
mod recovery;
#[path = "native/settings.rs"]
mod settings;
#[path = "native/ui.rs"]
mod ui;

fn main() {
    app::run();
}

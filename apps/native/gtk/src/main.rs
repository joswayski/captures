//! Captures' Linux host on GTK4: OS text, input and AT-SPI with Captures'
//! own token-drawn surfaces and controls. Runs beside the wgpu workbench
//! until it passes the feature, screenshot, performance and accessibility
//! gates in `docs/native-rewrite.md`.
mod preferences;
mod style;
mod switch;
mod tokens;
mod widgets;

use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib, graphene};

struct Options {
    scene: String,
    appearance: Option<String>,
    settings_file: PathBuf,
    history_root: Option<PathBuf>,
    screenshot: Option<PathBuf>,
    size: Option<(i32, i32)>,
}

fn parse(args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options {
        scene: "preferences".into(),
        appearance: None,
        settings_file: captures_settings::default_native_settings_path(),
        history_root: None,
        screenshot: None,
        size: None,
    };
    let mut args = args.skip(1);
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--scene" => options.scene = value()?,
            "--appearance" => options.appearance = Some(value()?),
            "--settings-file" => options.settings_file = value()?.into(),
            "--history-root" => options.history_root = Some(value()?.into()),
            "--screenshot" => options.screenshot = Some(value()?.into()),
            "--size" => {
                let size = value()?;
                let (w, h) = size.split_once('x').ok_or("--size is WIDTHxHEIGHT")?;
                options.size = Some((
                    w.parse().map_err(|_| "bad width")?,
                    h.parse().map_err(|_| "bad height")?,
                ));
            }
            _ => return Err(format!("unknown argument {flag}")),
        }
    }
    if options.scene != "preferences" {
        return Err(format!("unsupported scene {}", options.scene));
    }
    Ok(options)
}

fn main() -> glib::ExitCode {
    let options = match parse(std::env::args()) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            return glib::ExitCode::from(2);
        }
    };
    let app = gtk::Application::builder()
        .application_id("es.captur.Captures.Gtk")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let options = Rc::new(std::cell::RefCell::new(Some(options)));
    app.connect_activate(move |app| {
        let Some(options) = options.borrow_mut().take() else {
            return;
        };
        let settings = captures_settings::load(&options.settings_file).unwrap_or_default();
        let appearance = options.appearance.clone().unwrap_or_else(|| {
            let dark =
                gtk::Settings::default().is_some_and(|s| s.is_gtk_application_prefer_dark_theme());
            match settings.appearance {
                captures_settings::Appearance::Light => "light".into(),
                captures_settings::Appearance::Dark => "dark".into(),
                _ if dark => "dark".into(),
                _ => "light".into(),
            }
        });
        let theme = serde_json::to_value(settings.theme)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| "mustard".into());
        let tokens = Rc::new(tokens::Tokens::variant(&appearance, &theme));
        style::install(&tokens);
        let window = preferences::build(
            app,
            tokens,
            preferences::Options {
                settings_file: options.settings_file,
                history_root: options.history_root,
            },
        );
        if let Some((w, h)) = options.size {
            window.set_default_size(w, h);
        }
        window.present();
        if let Some(path) = options.screenshot {
            let window = window.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(600), move || {
                if let Err(error) = screenshot(&window, &path) {
                    eprintln!("screenshot failed: {error}");
                }
                window.application().expect("app").quit();
            });
        }
    });
    app.run_with_args::<&str>(&[])
}

/// Render the window's content (without decorations) to a PNG.
fn screenshot(window: &gtk::ApplicationWindow, path: &std::path::Path) -> Result<(), String> {
    let child = window.child().ok_or("no content")?;
    let (w, h) = (child.width() as f32, child.height() as f32);
    let paintable = gtk::WidgetPaintable::new(Some(&child));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, f64::from(w), f64::from(h));
    let node = snapshot.to_node().ok_or("empty frame")?;
    let renderer = window.renderer().ok_or("no renderer")?;
    let texture = renderer.render_texture(&node, Some(&graphene::Rect::new(0., 0., w, h)));
    texture.save_to_png(path).map_err(|e| e.to_string())
}

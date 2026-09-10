//! GTK rendering and worker delivery, shared by the native surfaces.
use gtk::{gdk, gdk_pixbuf::Pixbuf, glib, prelude::*};
use image::RgbaImage;
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

pub fn job<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
    done: impl FnOnce(Result<T, String>) + 'static,
) {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    let mut done = Some(done);
    glib::timeout_add_local(Duration::from_millis(16), move || {
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("Background operation stopped unexpectedly".into())
            }
        };
        if let Some(done) = done.take() {
            done(result);
        }
        glib::ControlFlow::Break
    });
}

pub fn pixbuf(image: &RgbaImage) -> Pixbuf {
    Pixbuf::from_bytes(
        &glib::Bytes::from_owned(image.as_raw().clone()),
        gtk::gdk_pixbuf::Colorspace::Rgb,
        true,
        8,
        image.width() as i32,
        image.height() as i32,
        image.width() as i32 * 4,
    )
}

pub fn button(text: &str) -> gtk::Button {
    gtk::Button::with_label(text)
}

pub fn label(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.style_context().add_class(class);
    label
}

pub fn save_bytes(directory: &Path, extension: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    for index in 0.. {
        let path = directory.join(format!("Capture-{stamp}-{index}.{extension}"));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
                    let _ = std::fs::remove_file(&path);
                    return Err(error.to_string());
                }
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    unreachable!()
}

pub fn save_png(image: &RgbaImage, directory: &Path) -> Result<PathBuf, String> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    save_bytes(directory, "png", bytes.get_ref())
}

pub fn error(parent: &gtk::Window, message: &str) {
    let dialog = gtk::MessageDialog::new(
        Some(parent),
        gtk::DialogFlags::MODAL,
        gtk::MessageType::Error,
        gtk::ButtonsType::Close,
        message,
    );
    dialog.run();
    dialog.close();
}

pub fn open_file(parent: &gtk::Window) -> Option<PathBuf> {
    let dialog = gtk::FileChooserDialog::new(
        Some("Open image or recording"),
        Some(parent),
        gtk::FileChooserAction::Open,
    );
    dialog.add_buttons(&[
        ("Cancel", gtk::ResponseType::Cancel),
        ("Open", gtk::ResponseType::Accept),
    ]);
    let result = (dialog.run() == gtk::ResponseType::Accept)
        .then(|| dialog.filename())
        .flatten();
    dialog.close();
    result
}

// Native CSS is generated from the same product tokens, not an independent palette.
fn token(name: &str) -> String {
    let name = match name {
        "accent" => "theme-accent",
        "signal" => "theme-signal",
        other => other,
    };
    let source = concat!(
        include_str!("../../../../shared/design.css"),
        "\n",
        include_str!("../../../../shared/themes.css")
    );
    let needle = format!("--{name}:");
    let value = source
        .split_once(&needle)
        .map(|(_, tail)| tail.split(';').next().unwrap().trim())
        .unwrap_or("#777777");
    if let Some(inner) = value
        .strip_prefix("var(--")
        .and_then(|s| s.strip_suffix(')'))
    {
        token(inner)
    } else {
        value.to_owned()
    }
}

pub fn color(name: &str) -> gdk::RGBA {
    gdk::RGBA::parse(&token(name)).unwrap_or(gdk::RGBA::BLACK)
}

pub fn install_theme(dark: bool) {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(dark);
    }
    let css = format!(
        r#"
        @define-color captures_accent {};
        @define-color captures_glass {};
        @define-color captures_glass_text {};
        * {{ font-family: Sans; font-size: {}; }}
        .title {{ font-size: {}; font-weight: 600; }}
        .muted {{ opacity: 0.65; }}
        button {{ border-radius: {}; padding: {} {}; }}
        button.primary {{ background-image: none; background-color: @captures_accent; color: {}; }}
        .toolbar {{ padding: {}; }}
        .glass {{ background-color: @captures_glass; color: @captures_glass_text; border-radius: {}; }}
        .glass label {{ color: @captures_glass_text; }}
        .glass button {{ background-image: none; background-color: {}; color: @captures_glass_text; border-color: {}; }}
        .glass button.primary, .glass button.primary label {{ background-color: @captures_accent; color: {}; }}
    "#,
        token("accent"),
        token("glass-strong"),
        token("glass-text"),
        token("text-md"),
        token("text-2xl"),
        token("r-md"),
        token("s-4"),
        token("s-5"),
        token("theme-accent-ink"),
        token("s-6"),
        token("r-xl"),
        token("glass-raised"),
        token("glass-border"),
        token("theme-accent-ink")
    );
    let provider = gtk::CssProvider::new();
    if let Err(error) = provider.load_from_data(css.as_bytes()) {
        eprintln!("Native theme: {error}");
    }
    if let Some(screen) = gdk::Screen::default() {
        gtk::StyleContext::add_provider_for_screen(
            &screen,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

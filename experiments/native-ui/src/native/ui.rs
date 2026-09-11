//! GTK rendering and worker delivery, shared by the native surfaces.
use gtk::{gdk, gdk_pixbuf::Pixbuf, glib, prelude::*};
use image::RgbaImage;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

#[path = "icons.rs"]
mod icons;

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

thread_local! {
    static TOKENS: RefCell<BTreeMap<String, String>> = RefCell::new(theme_tokens(false, "mustard", "", ""));
    static PROVIDER: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
}

// Parse only custom-property declarations. All visual values still come from the
// shared source of truth; GTK gets resolved values because it has no CSS vars.
fn variables(block: &str, values: &mut BTreeMap<String, String>) {
    let mut rest = block;
    while let Some(start) = rest.find("--") {
        rest = &rest[start + 2..];
        let Some(colon) = rest.find(':') else { break };
        let key = &rest[..colon];
        if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            continue;
        }
        let Some(end) = rest[colon + 1..].find(';') else {
            break;
        };
        values.insert(
            key.to_owned(),
            rest[colon + 1..colon + 1 + end].trim().to_owned(),
        );
        rest = &rest[colon + end + 2..];
    }
}

fn theme_tokens(dark: bool, theme: &str, accent: &str, signal: &str) -> BTreeMap<String, String> {
    let design = include_str!("../../../../shared/design.css");
    let themes = include_str!("../../../../shared/themes.css");
    let mut values = BTreeMap::new();
    let base = design
        .split_once(":root {")
        .unwrap()
        .1
        .split('}')
        .next()
        .unwrap();
    variables(base, &mut values);
    let palette = if dark {
        "[data-appearance=\"dark\"] {"
    } else {
        "[data-appearance=\"light\"] {"
    };
    variables(
        design
            .split_once(palette)
            .unwrap()
            .1
            .split('}')
            .next()
            .unwrap(),
        &mut values,
    );
    let selector = if theme == "mustard" || theme == "custom" {
        "[data-capture-theme=\"custom\"] {".to_owned()
    } else {
        format!("[data-capture-theme=\"{theme}\"] {{")
    };
    let block = themes
        .split_once(&selector)
        .unwrap_or_else(|| {
            themes
                .split_once("[data-capture-theme=\"custom\"] {")
                .unwrap()
        })
        .1;
    variables(block.split('}').next().unwrap(), &mut values);
    if theme == "custom" {
        let inks = [
            values["theme-accent-ink"].clone(),
            values["glass-text"].clone(),
        ];
        for (role, value) in [("accent", accent), ("signal", signal)] {
            if let Ok(color) = gdk::RGBA::parse(value) {
                values.insert(format!("theme-{role}"), value.to_owned());
                values.insert(
                    format!("theme-{role}-rgb"),
                    format!(
                        "{}, {}, {}",
                        (color.red() * 255.).round() as u8,
                        (color.green() * 255.).round() as u8,
                        (color.blue() * 255.).round() as u8
                    ),
                );
                // Use linear-light contrast, not perceived brightness: saturated
                // red, for example, needs dark ink even though its luminance is low.
                let luminance = |c: gdk::RGBA| {
                    [c.red(), c.green(), c.blue()]
                        .into_iter()
                        .zip([0.2126, 0.7152, 0.0722])
                        .map(|(v, w)| {
                            w * if v <= 0.03928 {
                                v / 12.92
                            } else {
                                ((v + 0.055) / 1.055).powf(2.4)
                            }
                        })
                        .sum::<f64>()
                };
                let bg = luminance(color);
                let ink = inks
                    .clone()
                    .into_iter()
                    .max_by(|a, b| {
                        let contrast = |s: &str| {
                            let fg = luminance(gdk::RGBA::parse(s).unwrap());
                            (bg.max(fg) + 0.05) / (bg.min(fg) + 0.05)
                        };
                        contrast(a).total_cmp(&contrast(b))
                    })
                    .unwrap();
                values.insert(format!("theme-{role}-ink"), ink);
            }
        }
    }
    values
}

fn resolve(value: &str, values: &BTreeMap<String, String>) -> String {
    fn expand(value: &str, values: &BTreeMap<String, String>, depth: usize) -> String {
        assert!(depth < 32, "Cyclic shared design token");
        let mut rest = value;
        let mut output = String::new();
        while let Some(start) = rest.find("var(--") {
            output.push_str(&rest[..start]);
            let end = rest[start..]
                .find(')')
                .map(|i| start + i)
                .expect("Unclosed design token");
            let key = &rest[start + 6..end];
            output.push_str(&expand(
                values
                    .get(key)
                    .unwrap_or_else(|| panic!("Missing token: {key}")),
                values,
                depth + 1,
            ));
            rest = &rest[end + 1..];
        }
        output.push_str(rest);
        output
    }
    expand(value, values, 0)
}

pub fn token(name: &str) -> String {
    let name = match name {
        "accent" => "theme-accent",
        "signal" => "theme-signal",
        other => other,
    };
    TOKENS.with_borrow(|values| resolve(values.get(name).map(String::as_str).unwrap_or(""), values))
}

pub fn color(name: &str) -> gdk::RGBA {
    gdk::RGBA::parse(&token(name)).unwrap_or(gdk::RGBA::BLACK)
}

pub fn install_theme_custom(dark: bool, theme: &str, accent: &str, signal: &str) {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(dark);
    }
    TOKENS.set(theme_tokens(dark, theme, accent, signal));
    // Pango resolves a comma-separated font stack through Fontconfig, which can
    // fall back on a missing first family before reaching the later CSS choices.
    // Select the first installed family ourselves, preserving browser order.
    // Arial is a Fontconfig-compatible alias (e.g. Nimbus Sans on Debian).
    if let Some(map) = gtk::Label::new(None).pango_context().font_map() {
        let families = map.list_families();
        TOKENS.with_borrow_mut(|values| {
            let stack = values.get("font-sans").expect("shared font stack");
            let family = stack
                .split(',')
                .map(|family| family.trim().trim_matches('"'))
                .find(|family| {
                    *family == "Arial"
                        || *family == "sans-serif"
                        || families
                            .iter()
                            .any(|f| f.name().eq_ignore_ascii_case(family))
                })
                .unwrap_or("sans-serif");
            let resolved = format!("\"{family}\"");
            values.insert("font-sans".into(), resolved);
        });
    }
    let css = TOKENS.with_borrow(|values| resolve(include_str!("style.css"), values));
    let provider = gtk::CssProvider::new();
    if let Err(error) = provider.load_from_data(css.as_bytes()) {
        eprintln!("Native theme: {error}");
    }
    if let Some(screen) = gdk::Screen::default() {
        PROVIDER.with_borrow_mut(|current| {
            if let Some(previous) = current.take() {
                gtk::StyleContext::remove_provider_for_screen(&screen, &previous);
            }
            *current = Some(provider.clone());
        });
        gtk::StyleContext::add_provider_for_screen(
            &screen,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

/// Draw the product's SVG outlines at device scale, inheriting state/theme color.
/// Cache the decoded pixels until the color or scale changes, not on every frame.
pub fn icon(name: &str, size: i32) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    // Like GtkImage, an icon must let its host button receive pointer events.
    area.set_has_window(false);
    area.set_size_request(size, size);
    area.set_halign(gtk::Align::Center);
    area.set_valign(gtk::Align::Center);
    let name = name.to_owned();
    let cache = RefCell::new(None::<(gdk::RGBA, i32, Pixbuf)>);
    area.connect_draw(move |area, cr| {
        let color = area.style_context().color(area.state_flags());
        let scale = area.scale_factor();
        let mut cache = cache.borrow_mut();
        if cache.as_ref().is_none_or(|(c, s, _)| *c != color || *s != scale) {
            if let Some(body) = icons::body(&name) {
                let color_css = format!("rgb({},{},{})", (color.red() * 255.).round(), (color.green() * 255.).round(), (color.blue() * 255.).round());
                let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{pixels}" height="{pixels}" viewBox="0 0 24 24" fill="none" stroke="{color_css}" color="{color_css}" opacity="{alpha}" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"#, pixels=size*scale, alpha=color.alpha());
                let loader = gtk::gdk_pixbuf::PixbufLoader::with_type("svg").expect("GTK SVG loader");
                if loader.write(svg.as_bytes()).is_ok() && loader.close().is_ok()
                    && let Some(pixbuf) = loader.pixbuf() {
                    *cache = Some((color, scale, pixbuf));
                }
            } else if let Some(theme) = gtk::IconTheme::default()
                && let Ok(Some(pixbuf)) = theme.load_icon(&name, size * scale, gtk::IconLookupFlags::FORCE_SIZE) {
                *cache = Some((color, scale, pixbuf));
            }
        }
        if let Some((_, _, pixbuf)) = cache.as_ref() {
            let _ = cr.save();
            cr.scale(1. / scale as f64, 1. / scale as f64);
            cr.set_source_pixbuf(pixbuf, 0., 0.);
            let _ = cr.paint();
            let _ = cr.restore();
        }
        glib::Propagation::Stop
    });
    area
}

pub fn icon_button(label: &str, name: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.set_image(Some(&icon(name, 16)));
    button.set_always_show_image(true);
    button.set_tooltip_text(Some(label));
    button.style_context().add_class("icon-button");
    if let Some(accessible) = button.accessible() {
        accessible.set_name(label);
    }
    button
}

pub fn named(widget: &impl IsA<gtk::Widget>, name: &str) {
    if let Some(accessible) = widget.accessible() {
        accessible.set_name(name);
    }
}

#[cfg(test)]
mod theme_tests {
    use super::*;
    #[test]
    fn custom_ink_contrasts_with_saturated_colors_independently_of_other_role() {
        for dark in [false, true] {
            let colors = theme_tokens(dark, "custom", "#0000ff", "#ff0000");
            assert_eq!(colors["theme-accent-ink"], "#f6f6f8");
            assert_eq!(colors["theme-signal-ink"], "#17181b");
        }
    }
    #[test]
    fn palettes_resolve_the_shared_light_dark_and_accent_sources() {
        let light = theme_tokens(false, "mustard", "", "");
        let dark = theme_tokens(true, "cobalt", "", "");
        assert_eq!(resolve("var(--surface-raised)", &light), "#ffffff");
        assert_eq!(resolve("var(--surface-raised)", &dark), "#16161b");
        assert_eq!(resolve("var(--text)", &light), "#131318");
        assert_ne!(dark["theme-accent"], light["theme-accent"]);
        assert!(!resolve(include_str!("style.css"), &light).contains("var(--"));
        assert_eq!(dark["glass-text"], light["glass-text"]);
    }
}

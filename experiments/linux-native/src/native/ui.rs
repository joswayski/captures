//! GTK rendering and worker delivery, shared by the native surfaces.
use crate::compat::prelude::*;
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

pub fn texture(image: &RgbaImage) -> gdk::Texture {
    gdk::MemoryTexture::new(
        image.width() as i32,
        image.height() as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from_owned(image.as_raw().clone()),
        image.width() as usize * 4,
    )
    .upcast()
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

pub fn install_provider(provider: &gtk::CssProvider, priority: u32) {
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("graphical display"),
        provider,
        priority,
    );
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
    let (dialog, actions) = notice(parent, "Something went wrong", message);
    let close = button("Close");
    close.style_context().add_class("primary");
    actions.append(&close);
    let dialog_for_close = dialog.clone();
    close.connect_clicked(move |_| dialog_for_close.close());
    dialog.present();
}

/// Present a non-blocking, token-styled destructive confirmation.
///
/// GTK4 deliberately removed the nested synchronous dialog API. Keeping the
/// confirmation in a regular transient window also prevents the desktop theme
/// from replacing Captures' visual language with a stock toolkit dialog.
pub fn confirm(
    parent: &gtk::Window,
    title: &str,
    message: &str,
    confirm_label: &str,
    confirmed: impl FnOnce() + 'static,
) {
    let (dialog, actions) = notice(parent, title, message);
    let cancel = button("Cancel");
    let accept = button(confirm_label);
    accept.style_context().add_class("destructive");
    actions.append(&cancel);
    actions.append(&accept);

    let dialog_for_cancel = dialog.clone();
    cancel.connect_clicked(move |_| dialog_for_cancel.close());
    let dialog_for_accept = dialog.clone();
    let confirmed = RefCell::new(Some(confirmed));
    accept.connect_clicked(move |_| {
        dialog_for_accept.close();
        if let Some(confirmed) = confirmed.borrow_mut().take() {
            confirmed();
        }
    });
    dialog.present();
}

fn notice(parent: &gtk::Window, title: &str, message: &str) -> (gtk::Window, gtk::Box) {
    let (dialog, content, actions) = panel(parent, title);
    let body = label(message, "notice-message");
    body.set_wrap(true);
    body.set_max_width_chars(48);
    content.append(&body);
    (dialog, actions)
}

pub fn panel(parent: &gtk::Window, title: &str) -> (gtk::Window, gtk::Box, gtk::Box) {
    let dialog = gtk::Window::new();
    dialog.set_title(Some(title));
    dialog.set_transient_for(Some(parent));
    dialog.set_modal(true);
    dialog.set_decorated(false);
    dialog.set_resizable(false);
    dialog.style_context().add_class("notice-window");

    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.style_context().add_class("notice-card");
    let heading = label(title, "notice-title");
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.style_context().add_class("notice-content");
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    actions.set_halign(gtk::Align::End);
    actions.style_context().add_class("notice-actions");
    card.append(&heading);
    card.append(&content);
    card.append(&actions);
    dialog.set_child(Some(&card));
    (dialog, content, actions)
}

pub fn open_file(parent: &gtk::Window, selected: impl FnOnce(PathBuf) + 'static) {
    let dialog = gtk::FileChooserNative::new(
        Some("Open image or recording"),
        Some(parent),
        gtk::FileChooserAction::Open,
        Some("Open"),
        Some("Cancel"),
    );
    let selected = RefCell::new(Some(selected));
    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Accept
            && let Some(path) = dialog.file().and_then(|file| file.path())
            && let Some(selected) = selected.borrow_mut().take()
        {
            selected(path);
        }
        dialog.destroy();
    });
    dialog.show();
}

pub fn choose_folder(
    parent: &gtk::Window,
    title: &str,
    initial: &Path,
    selected: impl FnOnce(PathBuf) + 'static,
) {
    let dialog = gtk::FileChooserNative::new(
        Some(title),
        Some(parent),
        gtk::FileChooserAction::SelectFolder,
        Some("Choose"),
        Some("Cancel"),
    );
    let _ = dialog.set_current_folder(Some(&gtk::gio::File::for_path(initial)));
    let selected = RefCell::new(Some(selected));
    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Accept
            && let Some(path) = dialog.file().and_then(|file| file.path())
            && let Some(selected) = selected.borrow_mut().take()
        {
            selected(path);
        }
        dialog.destroy();
    });
    dialog.show();
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
                        (f64::from(color.red()) * 255.).round() as u8,
                        (f64::from(color.green()) * 255.).round() as u8,
                        (f64::from(color.blue()) * 255.).round() as u8
                    ),
                );
                // Use linear-light contrast, not perceived brightness: saturated
                // red, for example, needs dark ink even though its luminance is low.
                let luminance = |c: gdk::RGBA| {
                    [
                        f64::from(c.red()),
                        f64::from(c.green()),
                        f64::from(c.blue()),
                    ]
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
                // Match shared/themes.ts interactiveShade: move away from the
                // chosen ink, preserving contrast instead of hovering mustard.
                let light_ink = ink == inks[1];
                let toward = if light_ink {
                    gdk::RGBA::BLACK
                } else {
                    gdk::RGBA::parse(&inks[1]).unwrap()
                };
                let mix = |from: gdk::RGBA, amount: f64| {
                    let channel = |a: f64, b: f64| ((a + (b - a) * amount) * 255.).round() / 255.;
                    gdk::RGBA::new(
                        channel(f64::from(from.red()), f64::from(toward.red())) as f32,
                        channel(f64::from(from.green()), f64::from(toward.green())) as f32,
                        channel(f64::from(from.blue()), f64::from(toward.blue())) as f32,
                        1.,
                    )
                };
                let candidate = mix(color, if light_ink { 0.06 } else { 0.1 });
                let fg = luminance(gdk::RGBA::parse(&ink).unwrap());
                let hover = (0..=25)
                    .map(|step| mix(candidate, step as f64 * 0.04))
                    .find(|color| {
                        let bg = luminance(*color);
                        (bg.max(fg) + 0.05) / (bg.min(fg) + 0.05) >= 4.5
                    })
                    .unwrap_or(toward);
                values.insert(
                    format!("theme-{role}-hover"),
                    format!(
                        "#{:02x}{:02x}{:02x}",
                        (f64::from(hover.red()) * 255.).round() as u8,
                        (f64::from(hover.green()) * 255.).round() as u8,
                        (f64::from(hover.blue()) * 255.).round() as u8
                    ),
                );
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
    provider.load_from_data(&css);
    PROVIDER.with_borrow_mut(|current| {
        *current = Some(provider.clone());
    });
    install_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
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
        let color = area.style_context().color();
        let scale = area.scale_factor();
        let mut cache = cache.borrow_mut();
        if cache.as_ref().is_none_or(|(c, s, _)| *c != color || *s != scale) {
            if let Some(body) = icons::body(&name) {
                let color_css = format!("rgb({},{},{})", (f64::from(color.red()) * 255.).round(), (f64::from(color.green()) * 255.).round(), (f64::from(color.blue()) * 255.).round());
                let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{pixels}" height="{pixels}" viewBox="0 0 24 24" fill="none" stroke="{color_css}" color="{color_css}" opacity="{alpha}" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"#, pixels=size*scale, alpha=f64::from(color.alpha()));
                let loader = gtk::gdk_pixbuf::PixbufLoader::with_type("svg").expect("GTK SVG loader");
                if loader.write(svg.as_bytes()).is_ok() && loader.close().is_ok()
                    && let Some(pixbuf) = loader.pixbuf() {
                    *cache = Some((color, scale, pixbuf));
                }
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
    named(&button, label);
    button
}

pub fn named(widget: &impl IsA<gtk::Widget>, name: &str) {
    let widget: &gtk::Widget = widget.as_ref();
    widget.update_property(&[gtk::accessible::Property::Label(name)]);
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
            assert_eq!(colors["theme-accent-hover"], "#0000f0");
            assert_eq!(colors["theme-signal-hover"], "#fe1919");
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
        assert_eq!(dark["glass-text"], light["glass-text"]);
    }

    #[test]
    fn every_gtk_stylesheet_token_resolves_in_both_appearances() {
        let stylesheet = include_str!("style.css");
        for (appearance, tokens) in [
            ("light", theme_tokens(false, "mustard", "", "")),
            ("dark", theme_tokens(true, "mustard", "", "")),
        ] {
            let resolved = resolve(stylesheet, &tokens);
            assert!(
                !resolved.contains("var(--"),
                "{appearance} GTK stylesheet contains an unresolved token"
            );
        }
    }
}

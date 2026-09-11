//! A native-only history index. Expiration removes metadata, never saved captures.
use crate::{
    settings::{atomic_write, data_dir},
    ui,
};
use gtk::{gio, prelude::*};
use serde::{Deserialize, Serialize};
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
    time::{SystemTime, UNIX_EPOCH},
};

const RETENTION_MS: u64 = 30 * 24 * 60 * 60 * 1000;
#[derive(Serialize, Deserialize)]
struct Entry {
    path: PathBuf,
    saved_ms: u64,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn load() -> Result<Vec<Entry>, String> {
    match std::fs::read(data_dir().join("history.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // One-time migration of the earlier native directory-based library.
            let directory = crate::settings::Settings::load()?.output_directory;
            let files = match std::fs::read_dir(directory) {
                Ok(files) => files,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
                Err(e) => return Err(e.to_string()),
            };
            let mut entries = Vec::new();
            for file in files {
                let file = file.map_err(|e| e.to_string())?;
                let path = file.path();
                if !path.is_file()
                    || path
                        .file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with("native-"))
                    || !path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                        ["png", "jpg", "jpeg", "webp", "gif", "mp4", "webm"]
                            .contains(&e.to_ascii_lowercase().as_str())
                    })
                {
                    continue;
                }
                let saved_ms = file
                    .metadata()
                    .and_then(|m| m.modified())
                    .map_err(|e| e.to_string())?
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                entries.push(Entry { path, saved_ms });
            }
            retain(&mut entries, now());
            atomic_write(
                &data_dir().join("history.json"),
                &serde_json::to_vec(&entries).map_err(|e| e.to_string())?,
            )?;
            Ok(entries)
        }
        Err(e) => Err(e.to_string()),
    }
}
fn retain(entries: &mut Vec<Entry>, time: u64) {
    entries.retain(|e| time.saturating_sub(e.saved_ms) <= RETENTION_MS && e.path.is_file());
}
fn save(entries: &[Entry]) -> Result<(), String> {
    atomic_write(
        &data_dir().join("history.json"),
        &serde_json::to_vec(entries).map_err(|e| e.to_string())?,
    )
}
fn remove_entry(entries: &mut Vec<Entry>, path: &Path) {
    entries.retain(|entry| entry.path != path);
}

fn update_filter_labels(filters: &[gtk::ToggleButton], counts: &[usize; 4]) {
    for (index, label) in ["All", "Screenshots", "Video", "GIF"]
        .into_iter()
        .enumerate()
    {
        filters[index].set_label(&format!("{label}   {}", counts[index]));
        ui::named(&filters[index], &format!("{label} {}", counts[index]));
        filters[index].set_sensitive(index == 0 || counts[index] > 0);
    }
}
pub fn record(path: &Path) -> Result<(), String> {
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    let mut entries = load()?;
    retain(&mut entries, now());
    entries.retain(|e| e.path != path);
    entries.push(Entry {
        path,
        saved_ms: now(),
    });
    save(&entries)
}

pub fn forget(path: &Path) -> Result<(), String> {
    let mut entries = load()?;
    remove_entry(&mut entries, path);
    save(&entries)
}

pub fn clear() -> Result<(), String> {
    save(&[])
}

/// Opens history with separate callbacks for opening a file and restoring an
/// image to the saved-preview stack. History actions never modify saved files.
pub fn open_with_restore(
    _directory: PathBuf,
    open: Rc<dyn Fn(PathBuf)>,
    restore: Rc<dyn Fn(PathBuf)>,
) {
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("Captures — History");
    window.set_default_size(1120, 720);
    let provider = gtk::CssProvider::new();
    provider
        .load_from_data(include_bytes!("history.css"))
        .expect("valid history CSS");
    if let Some(screen) = gtk::gdk::Screen::default() {
        gtk::StyleContext::add_provider_for_screen(
            &screen,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
    }
    let root = gtk::Box::new(gtk::Orientation::Vertical, 16);
    root.style_context().add_class("native-history");
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 24);
    header.style_context().add_class("native-history-header");
    let heading = gtk::Box::new(gtk::Orientation::Vertical, 6);
    heading.set_hexpand(true);
    heading.pack_start(
        &ui::label("ON THIS DEVICE", "native-history-eyebrow"),
        false,
        false,
        0,
    );
    heading.pack_start(
        &ui::label("Capture History", "native-history-title"),
        false,
        false,
        0,
    );
    let description = ui::label(
        "Screenshots, videos, GIFs, and interrupted recordings you can recover all appear here for 30 days.",
        "muted",
    );
    description.set_line_wrap(true);
    heading.pack_start(&description, false, false, 0);
    header.pack_start(&heading, true, true, 0);
    let clear_button = ui::button("Delete all");
    clear_button
        .style_context()
        .add_class("native-history-clear");
    ui::named(&clear_button, "Delete all captures");
    header.pack_end(&clear_button, false, false, 0);
    root.pack_start(&header, false, false, 0);

    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    toolbar.style_context().add_class("native-history-toolbar");
    ui::named(&toolbar, "Filter captures");
    let filters = Rc::new(RefCell::new(Vec::<gtk::ToggleButton>::new()));
    for label in ["All", "Screenshots", "Video", "GIF"] {
        let button = gtk::ToggleButton::with_label(label);
        button.style_context().add_class("native-history-filter");
        ui::named(&button, label);
        toolbar.pack_start(&button, false, false, 0);
        filters.borrow_mut().push(button);
    }
    root.pack_start(&toolbar, false, false, 0);
    let scroll = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    let flow = gtk::FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_min_children_per_line(1);
    flow.set_max_children_per_line(4);
    flow.set_row_spacing(16);
    flow.set_column_spacing(16);
    flow.set_valign(gtk::Align::Start);
    flow.style_context().add_class("native-history-grid");
    scroll.add(&flow);
    root.pack_start(&scroll, true, true, 0);
    let mut entries = match load() {
        Ok(entries) => entries,
        Err(error) => {
            ui::error(&window, &error);
            vec![]
        }
    };
    retain(&mut entries, now());
    entries.sort_by_key(|e| std::cmp::Reverse(e.saved_ms));
    clear_button.set_no_show_all(entries.is_empty());
    clear_button.set_visible(!entries.is_empty());
    toolbar.set_no_show_all(entries.is_empty());
    toolbar.set_visible(!entries.is_empty());
    let empty = gtk::Box::new(gtk::Orientation::Vertical, 10);
    empty.style_context().add_class("native-history-empty");
    empty.set_halign(gtk::Align::Center);
    empty.set_size_request(1000, 320);
    let empty_icon = gtk::Frame::new(None);
    empty_icon
        .style_context()
        .add_class("native-history-empty-icon");
    empty_icon.set_halign(gtk::Align::Center);
    empty_icon.set_size_request(52, 52);
    empty_icon.add(&ui::icon("history", 26));
    empty.pack_start(&empty_icon, false, false, 0);
    let empty_title = ui::label("No captures yet", "native-history-empty-title");
    empty_title.set_xalign(0.5);
    empty.pack_start(&empty_title, false, false, 0);
    let empty_copy = ui::label(
        "New screenshots, videos, and GIFs appear here automatically.",
        "muted",
    );
    empty_copy.set_xalign(0.5);
    empty.pack_start(&empty_copy, false, false, 0);
    ui::named(&empty, "No captures yet");
    flow.insert(&empty, -1);
    empty.parent().unwrap().set_visible(entries.is_empty());
    let cards = Rc::new(RefCell::new(Vec::<(gtk::Box, u32)>::new()));
    let kind = |path: &Path| match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "mp4" | "webm" => 2,
        "gif" => 3,
        _ => 1,
    };
    let counts = Rc::new(RefCell::new([
        entries.len(),
        entries.iter().filter(|e| kind(&e.path) == 1).count(),
        entries.iter().filter(|e| kind(&e.path) == 2).count(),
        entries.iter().filter(|e| kind(&e.path) == 3).count(),
    ]));
    update_filter_labels(&filters.borrow(), &counts.borrow());
    for entry in entries {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
        card.style_context().add_class("native-history-card");
        card.set_size_request(252, -1);
        card.set_halign(gtk::Align::Start);
        card.set_hexpand(false);
        let overlay = gtk::Overlay::new();
        overlay.set_size_request(252, 168);
        overlay.style_context().add_class("native-history-image");
        let image = gtk::Image::new();
        image.set_size_request(252, 168);
        let path = entry.path.clone();
        let thumb = image.clone();
        ui::job(
            move || {
                let image = if path
                    .extension()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| ["mp4", "webm"].contains(&s))
                {
                    let out = std::process::Command::new("ffmpeg")
                        .args(["-v", "error", "-i"])
                        .arg(&path)
                        .args([
                            "-frames:v",
                            "1",
                            "-vf",
                            "scale=252:168:force_original_aspect_ratio=decrease",
                            "-f",
                            "image2pipe",
                            "-vcodec",
                            "png",
                            "-",
                        ])
                        .output()
                        .map_err(|e| e.to_string())?;
                    image::load_from_memory(&out.stdout).map_err(|e| e.to_string())?
                } else {
                    image::open(path).map_err(|e| e.to_string())?
                };
                Ok(image.thumbnail(252, 168).to_rgba8())
            },
            move |result| {
                if let Ok(image) = result {
                    thumb.set_from_pixbuf(Some(&ui::pixbuf(&image)));
                }
            },
        );
        overlay.add(&image);
        card.add(&overlay);
        let name = entry
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let date = gtk::glib::DateTime::from_unix_local((entry.saved_ms / 1000) as i64)
            .and_then(|date| date.format("%b %-d, %Y, %-I:%M %p"))
            .map(|date| date.to_string())
            .unwrap_or_else(|_| name.clone());
        let label = ui::label(&date, "native-history-date");
        ui::named(&card, &format!("History item {name}"));
        label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        label.set_max_width_chars(28);
        label.set_margin_start(12);
        label.set_margin_end(12);
        card.add(&label);
        let ext = entry
            .path
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        let category = kind(&entry.path);
        let size = std::fs::metadata(&entry.path).map(|m| m.len()).unwrap_or(0);
        let metadata = ui::label(
            &format!(
                "{} · {:.1} MB",
                ext.to_ascii_uppercase(),
                size as f64 / 1_000_000.
            ),
            "muted",
        );
        metadata.set_margin_start(12);
        metadata.set_margin_end(12);
        card.add(&metadata);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        actions.style_context().add_class("native-history-actions");
        actions.set_margin_start(12);
        actions.set_margin_end(12);
        actions.set_margin_bottom(12);
        let button = ui::button("Edit");
        button.style_context().add_class("primary");
        button.set_size_request(111, -1);
        ui::named(&button, &format!("Edit {name}"));
        let action = open.clone();
        let path = entry.path.clone();
        button.connect_clicked(move |_| action(path.clone()));
        actions.pack_start(&button, true, true, 0);
        if !["mp4", "webm"].contains(&ext.as_str()) {
            let button = ui::button("Restore");
            button.set_size_request(111, -1);
            ui::named(&button, &format!("Restore {name}"));
            let action = restore.clone();
            let path = entry.path.clone();
            button.connect_clicked(move |_| action(path.clone()));
            actions.pack_start(&button, true, true, 0);
        } else {
            let folder = ui::button("Show in Folder");
            folder.set_size_request(111, -1);
            ui::named(&folder, &format!("Show {name} in Folder"));
            let entry_path = entry.path.clone();
            folder.connect_clicked(move |_| {
                if let Some(parent) = entry_path.parent() {
                    let _ = gio::AppInfo::launch_default_for_uri(
                        &gio::File::for_path(parent).uri(),
                        gio::AppLaunchContext::NONE,
                    );
                }
            });
            actions.pack_start(&folder, true, true, 0);
        }
        card.add(&actions);
        let remove = ui::icon_button("Delete from History", "trash");
        remove.style_context().add_class("native-history-delete");
        remove.set_halign(gtk::Align::End);
        remove.set_valign(gtk::Align::Start);
        remove.set_margin_top(6);
        remove.set_margin_end(6);
        ui::named(&remove, &format!("Delete {name} from History"));
        {
            let path = entry.path.clone();
            let card = card.clone();
            let flow = flow.clone();
            let empty = empty.clone();
            let cards = cards.clone();
            let window = window.clone();
            let toolbar = toolbar.clone();
            let clear_button = clear_button.clone();
            let filters = filters.clone();
            let counts = counts.clone();
            let confirming = Rc::new(RefCell::new(false));
            remove.connect_clicked(move |button| {
                if !*confirming.borrow() {
                    *confirming.borrow_mut() = true;
                    button.style_context().add_class("confirm");
                    ui::named(button, &format!("Confirm removal of {name} from History"));
                    return;
                }
                match forget(&path) {
                    Ok(()) => {
                        let mut counts = counts.borrow_mut();
                        counts[0] = counts[0].saturating_sub(1);
                        counts[category as usize] = counts[category as usize].saturating_sub(1);
                        update_filter_labels(&filters.borrow(), &counts);
                        drop(counts);
                        if let Some(child) = card.parent() {
                            flow.remove(&child);
                        }
                        cards
                            .borrow_mut()
                            .retain(|(candidate, _)| candidate != &card);
                        empty
                            .parent()
                            .unwrap()
                            .set_visible(cards.borrow().is_empty());
                        if cards.borrow().is_empty() {
                            toolbar.hide();
                            clear_button.hide();
                        }
                    }
                    Err(error) => ui::error(&window, &error),
                }
            });
        }
        overlay.add_overlay(&remove);
        flow.insert(&card, -1);
        cards.borrow_mut().push((card, category));
    }
    let active_filter = Rc::new(Cell::new(0usize));
    for (index, button) in filters.borrow().iter().enumerate() {
        let all_filters = filters.clone();
        let cards = cards.clone();
        let active_filter = active_filter.clone();
        button.connect_toggled(move |clicked| {
            if !clicked.is_active() {
                if active_filter.get() == index {
                    clicked.set_active(true);
                }
                return;
            }
            active_filter.set(index);
            for other in all_filters.borrow().iter() {
                if other != clicked {
                    other.set_active(false);
                }
            }
            for (card, category) in cards.borrow().iter() {
                if let Some(child) = card.parent() {
                    child.set_visible(index == 0 || *category == index as u32);
                }
            }
        });
    }
    filters.borrow()[0].set_active(true);
    {
        let flow = flow.clone();
        let empty = empty.clone();
        let cards = cards.clone();
        let toolbar = toolbar.clone();
        let filters = filters.clone();
        let counts = counts.clone();
        let confirming = Rc::new(RefCell::new(false));
        clear_button.connect_clicked(move |button| {
            if cards.borrow().is_empty() {
                return;
            }
            if !*confirming.borrow() {
                *confirming.borrow_mut() = true;
                button.set_label("Delete all forever");
                button.style_context().add_class("confirm");
                ui::named(button, "Confirm delete all captures");
                return;
            }
            match clear() {
                Ok(()) => {
                    *counts.borrow_mut() = [0; 4];
                    update_filter_labels(&filters.borrow(), &counts.borrow());
                    for (card, _) in cards.borrow_mut().drain(..) {
                        if let Some(child) = card.parent() {
                            flow.remove(&child);
                        }
                    }
                    empty.parent().unwrap().show();
                    toolbar.hide();
                    button.hide();
                }
                Err(error) => eprintln!("Could not clear history: {error}"),
            }
        });
    }
    window.add(&root);
    window.show_all();
    update_filter_labels(&filters.borrow(), &counts.borrow());
    empty
        .parent()
        .unwrap()
        .set_visible(cards.borrow().is_empty());
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retention_boundary_keeps_files_and_drops_only_expired_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.png");
        let b = dir.path().join("b.png");
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(&b, b"b").unwrap();
        let mut entries = vec![
            Entry {
                path: a.clone(),
                saved_ms: 5,
            },
            Entry {
                path: b.clone(),
                saved_ms: 6,
            },
        ];
        retain(&mut entries, RETENTION_MS + 6);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, b);
        assert!(a.is_file());
        assert!(b.is_file());
    }

    #[test]
    fn removal_forgets_only_the_selected_entry_and_preserves_saved_files() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.png");
        let b = dir.path().join("b.png");
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(&b, b"b").unwrap();
        let mut entries = vec![
            Entry {
                path: a.clone(),
                saved_ms: 1,
            },
            Entry {
                path: b.clone(),
                saved_ms: 2,
            },
        ];
        remove_entry(&mut entries, &a);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, b);
        assert_eq!(std::fs::read(a).unwrap(), b"a");
        assert_eq!(std::fs::read(&entries[0].path).unwrap(), b"b");
    }

    #[test]
    fn clearing_entries_preserves_every_saved_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("capture.png");
        std::fs::write(&path, b"capture").unwrap();
        let mut entries = vec![Entry {
            path: path.clone(),
            saved_ms: 1,
        }];
        entries.clear();
        assert!(entries.is_empty());
        assert_eq!(std::fs::read(path).unwrap(), b"capture");
    }
}
